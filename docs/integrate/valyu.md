---
title: Valyu configuration and limitations
domain: extensions
audience: integrator
section: reference
---

# Valyu configuration and limitations

Set `VALYU_API_KEY` in the environment or the operator's credentials file
(`commonmeasure credentials` names the variable). The adapter sends it only
in the `x-api-key` header to `https://api.valyu.ai`. An absent key produces an
unavailable result; there is no anonymous fallback.

`context_search` accepts `provider: "valyu"`, a query and a limit. A batch
suite may also list `valyu` in `providers`; `fetch_target` selects Contents
for one named URL. `context_fetch` does not select a supplier.

Search sends `POST /v1/search` with `search_type: "all"`, `is_tool_call: true`
and `max_num_results` clamped to 1–20. This includes web and proprietary
sources where the account permits them. Allowed hosts travel as
`included_sources`, a supplier-side hint; source policy still checks each
returned URL. The standard 20-result ceiling is declared so a batch job
asking for more records a coverage gap.

Fetch sends `POST /v1/contents` with one URL and `response_length: "medium"`
(up to 50,000 characters). No summary, structured extraction, asynchronous
job, Answer or DeepResearch is requested. The documented summary default is
false. Each operation makes one request, with no automatic retry or cache.

Each envelope keeps the returned markdown, URL and title. `publication_date`
is a supplier-declared date, unverified against publication; `crawl_date` and
`last_modified` remain native metadata. Source identifiers, descriptions,
scores, images, per-result `price` (USD) and extraction metadata also remain
native metadata. A licensed-corpus description does not state permitted uses:
licence rights stay unknown and only `search` and `fetch` are declared.

The search total comes from `total_deduction_dollars`; the Contents total
comes from `total_cost_dollars`. These become observed USD charges. Missing
totals stay unknown, even if individual results carry prices; prices are not
summed to invent a total. A disclosed amount finer than micro-dollar precision
stays in the native charge without a comparable monetary amount. An observed
zero is distinct from an absent charge.

A 206 response preserves the partial reply, offering only Contents items
whose `status` is `success`. Failed items stay in the exact raw response and
never supply text. An all-failed 422 is an explicit supplier error, not a
successful free acquisition. A false top-level `success`, missing results
array or unreadable Contents item status also fails explicitly.

## Verification

The adapter is spec-verified against the [Search reference](https://docs.valyu.ai/api-reference/endpoint/search.md)
and [Contents reference](https://docs.valyu.ai/api-reference/endpoint/contents.md),
OpenAPI 2.3.0. `crates/commonmeasure-supply/tests/valyu_spec.rs` exercises
synthetic documented shapes over loopback transport. Synthetic tests do not
establish live integration.

The recon jobs `demo/jobs/recon-valyu-search.json` and
`demo/jobs/recon-valyu-fetch.json` support recorded replay using the private
recon manifest. A replay reads retained, redacted responses through the
production adapter and seals the served bytes; it makes no supplier call.
Retained direct API success, recorded replay and live adapter verification
remain separate evidence states.

The supplier's AUP and Platform Service Agreement have not been verified by
this implementation. No unverified licence or publisher-reporting right is
inferred from the available website terms.
