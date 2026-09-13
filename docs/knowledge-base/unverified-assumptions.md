---
title: Unverified adapter assumptions
draft: true
---

# Unverified adapter assumptions

The register of what an implemented adapter assumes and a live run has not
confirmed. It shrinks as assumptions are verified; it is never deleted. It
is a dated register: each status names the date of the evidence that set it.

The verification evidence is in
[`docs/knowledge-base/provider-verification.md`](provider-verification.md) and `demo/recon/`.

## Evidence levels

The statuses below are drawn from the verification vocabulary in
[`docs/contracts/provider.md`](../contracts/provider.md) §Verification states, which is where each state is
defined — conflating `spec-verified`, `live-verified` and `replay-tested` is how
a green adapter reaches production wrong.

## Exa, Firecrawl, Tavily

`search` is live-verified for all three, with the recorded exchanges in
`demo/recon/`. What remains unverified:

| Ref | Assumption | Status |
|---|---|---|
| E1 | Exa's bundled page contents are free alongside the search charge | **Unverified.** One observation showed text for two results with no surcharge on the published search price. One observation is not a pricing rule |
| E2 | Exa's `PAYMENT-REQUIRED`, `PAYMENT-RESPONSE` and `Payment-Receipt` response headers | **Unknown.** They appear in `access-control-expose-headers`; the documentation URL returns 404. A `Payment-Receipt` would bear on the `corroborate` capability, so this is worth a follow-up. Nothing is claimed from a header name |
| F1 | Firecrawl's `creditsUsed` stays at the top level on `/v2/search` | **Spec-verified**, and live-verified once. It is nested at `data.metadata.creditsUsed` on `/v2/scrape`, which the published reference does not document. The adapter reads both positions |
| F2 | The credit-to-currency rate | **Unknowable from the API.** It depends on plan. Any dollar figure for Firecrawl is derived and must be labelled derived; the adapter produces none |
| T1 | Tavily's metered consumption is observable at all | **Contradicted so far.** Account counters did not move across three reads spanning a billed search. Not concluded broken; concluded unobservable within the window watched. The adapter records the published price as `quoted` |
| X1 | Rate limits and failure modes under load, for all three | **Unknown.** Two requests each says nothing about sustained behaviour. This is the gap between `live-verified` and `production-observed` |
| E3 | Exa declares `publishedDate` on search results, and the adapter maps it as the source's declared date | **Spec-verified only.** Neither recorded search result carried the field (`demo/recon/exa/` — the same optional metadata family as `author`, which the contents call shows arriving null), and the 5 August 2026 live run (`b819d775`, gitignored) observed the same absence on live bytes, so the mapping has still never fired outside the loopback spec test. Where a date does arrive it is Exa's claim about the page, untested against when the content was written |
| T2 | Tavily can declare a result date at all on the searches this adapter performs | **Not requested.** Its documented `published_date` exists only for news-topic searches; this adapter sends general-topic basic search, and the recorded response carries no date field. No date is mapped |
| F3 | Firecrawl search results carry a date | **Contradicted by everything seen.** No date field on any recorded search result and none documented for the endpoint. The scrape leg's `cachedAt` is a fact about Firecrawl's cache, not the content, and is deliberately not mapped as a date |

## TollBit

Spec-checked 2026-07-09 against TollBit's published OpenAPI document and
official Python SDK (`github.com/tollbit/tollbit-python-sdk`,
`spec/openapi.TollBit.Apis.yaml`), and probed live with a real key on
1 August 2026.

Only the search leg is implemented. Every priced operation was refused, so the
rate, token and content legs are not in the codebase at all; the assumptions
below are what an implementation would have to confirm first.

| Ref | Assumption | Status |
|---|---|---|
| U2 | The API key travels in a `TollbitKey` request header | **Live-verified 1 August 2026.** Six authenticated requests were accepted |
| U7 | Search exists and is usable | **Live-verified 1 August 2026.** `GET /dev/v2/search` returned 20 results in 914 ms with publisher and availability populated |
| U15 | `availability.readyToLicense` predicts a successful rate lookup | **Contradicted, twice out of two.** Both pages marked ready answered `403` at `/rates`. Two is a small sample; the claim is only that the flag cannot be relied upon |
| U16 | Whether the 403 at `/rates` is an account-level or page-level refusal | **Answered 4 August 2026: account-level.** TollBit's own quickstart demo publisher (`pioneervalleygazette.com/daydream`) returns the identical problem document, in both the recon URL form and the quickstart's bare form (`demo/recon/tollbit/tollbit-rates-demo-publisher.json`). A key refused the documentation's own demo target holds no page-level story. The prerequisite no probe has exercised is an AgentID registered in the developer dashboard, whose user agent the token request must carry |
| U3 | Licence-type vocabulary (`ON_DEMAND_LICENSE`, `ON_DEMAND_FULL_USE_LICENSE`, `CUSTOM_LICENSE`) and the mapping from a declared usage type | **Spec-verified only.** Crawl and training intent is expressed by endpoint, not by a licence value. The OpenAPI types `licenseType` as an unconstrained string, so the closed vocabulary is prose |
| U5 | Prices are integer micros alongside a currency string, and the licence identity and charged price arrive with the content rather than with the token | **Spec-verified only.** No TollBit price has ever been observed. Any micro-price in a past fixture was a fixture |
| U6 | The content endpoint tolerates a `Content-Telemetry-ID` request header | **Absent from the spec.** The string "telemetry" does not occur in the OpenAPI document, the SDK or the documentation pages read. TollBit's correlation runs through `User-Agent` and an `idempotencyId` on `selfReport`. This is the header the PARSE taskforce asked marketplaces to accept; no marketplace implements it |
| U13 | Rate limits | **Unknown.** Not documented. The SDK's default request timeout is five seconds |
| U14 | Whether a paid fetch requires funded billing, KYC or a sandbox, and the true per-fetch price floor | **Unknown; the open question.** Self-serve developer accounts exist; the documentation shows example prices of $0.005 and $0.01 but publishes no global minimum. Registration alone does not settle it: an AgentID was registered on 4 August 2026 and the re-probes were unchanged — rates 403, token creation a bodyless 500 in three request forms (`demo/recon/tollbit/tollbit-agent-probes.json`). What is left is the dashboard's payment settings, then TollBit support |
| U19 | The `publishedDate` on TollBit search items reflects when the content was published | **Field live-verified, claim untested.** Every recorded search item carried the field (1 August 2026, `demo/recon/tollbit/`) and the adapter maps it as the source's declared date — but the date is the supplier's claim about a page whose text TollBit does not return, so nothing can even be cross-read against it |
| U17 | That `/transactions/selfReport` is a citation-reporting mechanism | **Contradicted (spec-verified 2 August 2026).** It reports use counts and prices them: the receipt returns `perUnitPriceMicros`, `totalUsePriceMicros`, `currency` and `license`. The strings `citation`, `cited` and `attribut` do not occur in the document. It is a settlement mechanism with the buyer as the meter, not an attribution one. Deliberately not called: posting a transaction that did not occur would corrupt supplier-side evidence |

## TinyFish

Spec-checked 25 August 2026 against the published Search API reference
(`https://docs.tinyfish.ai/search-api/reference.md`); never called live, no
credential held. The supply-class posture is in the supply map, not here — this
table is only what the adapter's wire claims assume.

| Ref | Assumption | Status |
|---|---|---|
| TN1 | A search really bills the published zero at any wallet balance | **Spec-verified only.** The same reference documents `402 Payment required — Search API access is required for your account`, so "free" is conditional on account state. The quoted zero must never be displayed as observed |
| TN2 | The response carries no request identifier and no cost field | **Spec-verified only.** Claimed from the documented body shape; response headers are not documented and have never been seen |
| TN3 | `date` ever arrives on ordinary web results | **Untested.** Documented as "present for news results and some web results"; whether it fires on the default `domain_type=web` searches this adapter sends is unknown, the same family of doubt as E3/T2 |
| TN4 | `include_domains` restricts rather than merely re-ranks | **Untested.** The documented semantics are "restrict results to"; a provider treating it as a hint would return out-of-scope hosts, which admission would then refuse — visible, not silent, but worth confirming on first live contact |
| TN5 | No server-side result-count parameter exists | **Spec-verified only.** None is documented, so the adapter caps the offered candidates to the job's limit client-side. If a count field is verified later, move the cap onto the request |

## Ozone Live

Spec-checked 9 September 2026 against the machine-readable specification
(`https://api.ozone.live/openapi.yaml`) and the API reference; a credential is
held but no live call is recorded in this repository. This table is only what
the adapter's wire claims assume.

| Ref | Assumption | Status |
|---|---|---|
| OZ1 | `published_date` is the article's publication date | **Doubtful; spec-verified field, claim contradicted by the vendor's own example.** The reference's example result is a 2019 general-election article with `published_date: 2026-04-29` and `temporal_sensitivity: historical`, which reads as an ingestion or re-crawl date. The adapter maps the field with provenance naming it because it is the documented date; a freshness-weighted objective over Ozone supply is scoring the supplier's claim. One live call (9 September 2026, bytes not committed) showed both cases: a Guardian row whose `published_date` matched the date in its URL, and the same 2019 election article dated `2026-04-29`. Check against article bylines before trusting the field for freshness |
| OZ2 | `filters.domains` matches a job's full host names | **Untested.** Ozone's `domain` values are registrable domains (`bbc.com`, `coventrytelegraph.net`); a job names hosts (`www.bbc.com`). Whether the filter matches a subdomain against its registrable domain is undocumented. A miss returns nothing, which is visible; admission enforces the host policy either way |
| OZ3 | A search bills nothing observable | **Unknown.** No cost field on any response and no published price; `usage.embedding_tokens` is consumption, not a charge. Recorded unknown, never zero |
| OZ4 | The response carries no request identifier | **Spec-verified only.** Claimed from the documented body; response headers are not documented and have never been seen |
| OZ5 | `licensed: true` means Ozone holds a licence to serve the passage, not that the operator holds one to use it | **Reading of the specification, not verified with the supplier.** The specification's own description of the flag is "true only for corpus content". What the operator's agreement with Ozone permits is stated nowhere in the API and must come from the operator |
| OZ6 | `group_by: article` returns the best-scoring passage as the row's `text` | **Spec-verified only.** The specification says the row is "the best-scoring chunk, with the rest nested under `additional_chunks`"; which passage a freshness or grounding evaluator would prefer is a separate question the adapter does not answer |
| OZ7 | `/v1/contents` on a non-corpus URL performs a live fetch under Ozone's fetcher | **Spec-verified only.** The page is fetched by a third party on the operator's behalf, as it is for every fetch provider; the item is marked `source: live_fetch`, `licensed: false` and the adapter keeps both. The fetcher's identity and whether it honours the origin's declarations are undocumented |
| OZ8 | The `domain` field is the host of `url` | **Contradicted on one live row (9 September 2026, bytes not committed).** A `chroniclelive.co.uk` URL carried `domain: mirror.co.uk`, the publisher group's domain rather than the page's. The adapter derives the envelope host from `url` and keeps `domain` in native metadata; a source policy that read `domain` would allow or refuse the wrong host |
| OZ9 | `publisher_name` is resolved from the ownership manifest | **Not observed.** Empty on every live row although each carried a `publisher_id`; the reference's example shows it populated. Nothing in the adapter depends on it |
| OZ10 | The ownership manifest (`GET /v1/publishers`, `GET /v1/publishers/domains`) is served | **Not observed.** One authenticated call on 9 September 2026 answered `503 upstream_unavailable`: the ownership data had not been synced. OZ9's empty `publisher_name` is consistent with this. The shape the hub's network import expects is the documented one and has been exercised on a synthetic map only |

## Redpine

`live-verified` for MCP discovery and a preview→confirm licensed fetch under
trial (4 August 2026, `demo/recon/redpine/`). What remains unverified:

| Ref | Assumption | Status |
|---|---|---|
| R1 | Which price a funded account pays: the tool annotation (`cost_credits: 0.03`, `cost_type: "exact"`) or the payload's `_meta.cost_usd: 0.0015` | **Trial-observed 6 August 2026 (adapter live run `b8f0bcf2`, gitignored `demo/output/live-redpine/`, pre-registered in `demo/recon/redpine/NOTES.md`): a trial account is charged trial queries, not currency.** The receipt recorded `cost_charged: "0.000000"` while the meter moved 4 → 3, and both price figures were returned again unchanged and still twenty-fold apart. The funded-account price remains unknown; settled only by a post-trial query against a cash balance |
| R2 | That `cost_charged` and `balance_remaining` on `confirm` become real currency movements after the trial | **Trial-verified only, twice.** The meter moved (5→4 in recon, 4→3 in the 6 August adapter live run) but `cost_charged` was `0.000000` and `balance_remaining` `0.00` throughout |
| R3 | The REST `POST /api/v1/search/query` contract (`{collection, query}` → ranked results with `queryId`) | **Spec-read only.** Never called; no collection name is discoverable from the MCP surface |
| R4 | Permitted use of fetched licensed content (retention, quotation, redistribution) | **Not machine-readable anywhere in the responses.** The committed capture truncates the transcript text for this reason; terms need Redpine to state them. Addressed in behaviour: every Redpine acquisition carries `LicenceState::Unknown` explicitly, and `_meta.data_source` (a partnership name) is never promoted into a rights claim (`crates/commonmeasure-supply/src/redpine.rs`) |
| R5 | The preview shape outside trial coverage: the `billing_status` vocabulary beyond `"trial"`, and whether a funded preview carries a machine-readable price | **Unrecorded.** Every observed preview is trial-covered (the 6 August live run's non-fresh-trial note re-counted correctly: "4 of 5 … 3 would remain"). The declined-quote path is therefore exercised over a documented-shape construction (`crates/commonmeasure-runtime/tests/quoted_purchase.rs`), the Exa `publishedDate` precedent (E3); settled only by a preview against a funded or trial-exhausted account |
| R6 | That `media--sample`'s `filter.keyword` accepts only monitored keywords, and how it behaves on free text | **Untested.** The recorded query is the monitored keyword `NVIDIA` and the adapter maps the job prompt verbatim into `filter.keyword`; a sentence-shaped prompt is unverified behaviour, and no trial query will be spent discovering it |

## Skills

The local skill adapter is `live-verified` for `invoke` against two real
third-party bundles (6 August 2026, `demo/output/skills/`). What it assumes,
and what a supervised spawn cannot promise:

| Ref | Assumption | Status |
|---|---|---|
| S1 | The bundle digests identify what actually ran | **True at the read, not at the exec.** `SKILL.md` and the entrypoint are resolved and hashed immediately before the spawn, but a file replaced between the read and the `execve` would be recorded under the previous digest. Closing it needs an fd-based exec this runtime does not do; the window is microseconds and the operator owns the disk, so it is accepted and recorded |
| S2 | Killing the child's process group ends the invocation | **Live-verified for the ordinary case, 6 August 2026** — the defect that put it here was a probe whose shell had spawned `sleep 30`, where killing the child alone left the grandchild holding the output pipe and a 200 ms bound returned after thirty seconds. A descendant that escapes into its own process group, or that ignores `SIGKILL` as an uninterruptible-sleep process can, is beyond what the group signal reaches. Unmeasured: no probe has tried to escape |
| S3 | The declared output cap bounds this runtime's memory | **Approximately.** Reading stops one chunk past the cap, so the peak is the cap plus at most 8 KiB per stream, and the result is then refused whole. It is a bound on what is retained, not a guarantee about what a hostile program can make this process allocate transiently |
| S4 | An emptied environment is meaningful isolation | **Partly, and only against one thing.** It stops the child reading the operator's provider credentials out of the environment, which is what it is for. It is not a sandbox: the child runs as the operator, with the operator's filesystem and network access, and can read a credential from a file as easily as this runtime can. [`docs/contracts/provider.md`](../contracts/provider.md) says so where a reader would otherwise assume more |
| S5 | The frontmatter allow-list is `name`, `description`, `license`, `allowed-tools`, `metadata` | **Verified against the implementation, 6 August 2026**: read from the validator's own `allowed_properties` set, not from documentation. It is why `declared_version` is null in every invocation record. A format that later adds a version key would make that null wrong, and nothing here would notice |
| S6 | `SKILL.md`'s identity keys can be read without a YAML parser | **Assumed, deliberately.** The adapter reads only top-level `name:` and `license:` as plain scalars and treats anything else as absent. A quoted, folded or anchored value would be recorded wrong or not at all — recorded as absent in the common case. It records a third party's claim rather than validating it, so a mis-read is a missing declaration, never a wrong admission |
| S7 | A skill's result belongs to standard output | **Convention, not contract.** Nothing in the format says where a result goes. Both surveyed validators print to stdout; a skill that answers on stderr produces no admissible supply here, and the record shows the byte counts of both streams so a reader can see that is what happened |
| S8 | Repointing a catalogue entry is detectable | **Only by comparing digests between runs.** The manifest seals that a skill provider ran, not which bytes; a third party's bundle on the operator's disk is not this repository's code, so it is observed per run rather than sealed as experiment identity ([`docs/contracts/run-output.md`](../contracts/run-output.md) §Current limitations). Two runs of one suite against a swapped bundle therefore share a manifest hash and differ in `acquisition.invocation` |

## Transport

| Ref | Assumption | Status |
|---|---|---|
| U11 | Live endpoints are HTTPS and the client can reach them | **Closed.** `commonmeasure-http` speaks `https` via `rustls` with a vendored Mozilla root set and no way to disable verification. The offline proof runs in the default suite: `crates/commonmeasure-http/src/tls.rs::tests::u11_the_client_is_built_for_verified_https` builds the client config from the vendored roots, parses it once per process, and refuses a host that is not a TLS server name before any socket opens. Reaching a real origin needs the network, so that half is the ignored test `commonmeasure-http/tests/roundtrip.rs::https_reaches_a_real_origin`; run it with `cargo test -p commonmeasure-http -- --ignored` |
| U18 | A conforming Content Telemetry receiver accepts the relay's projection, deduplicates redelivered event ids, and scopes each owner's view to its own content | **Verified live 2 August 2026** against a conforming receiver (oa-server) running locally with a provisioned demo database. The standing proof is the ignored test `commonmeasure-cli/tests/relay_e2e.rs::a_live_oa_server_accepts_the_projection_and_isolates_owners`; its doc comment names the environment variables it needs. Offline tests use a real loopback `commonmeasure_http::Server` to observe the posted bytes, never a mock of the receiver's logic |
