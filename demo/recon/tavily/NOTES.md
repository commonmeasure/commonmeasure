# Tavily reconnaissance notes, 1 August 2026

Probed 1 August 2026. Four requests: one billable search and three reads of the
usage endpoint.

## The charge is reported nowhere

This is the significant finding. Tavily's search response carries **no cost
field and no credit field**. That alone would be unremarkable — but the account
usage endpoint does not fill the gap either.

`GET /usage` was read three times: before the billed search, immediately after,
and again several minutes later. All three reads returned identical counters:

```json
"key":     { "usage": 0, "search_usage": 0, "limit": 1000 }
"account": { "plan_usage": 0, "paygo_usage": 0, "current_plan": "Researcher" }
```

A search that returned 200 with two results never appeared in any counter
during the observation window. So for Tavily, acquisition cost is **quoted
only**: 1 credit for `search_depth: "basic"`, 2 for `advanced`, at a published
USD 0.008 per credit on pay-as-you-go.

`docs/contracts/experiment.md` requires quoted and observed cost to be
distinguished. Tavily is the case where they cannot be reconciled at all, and
the demo should show that as a gap rather than quietly printing the quoted
number as though it had been measured. It is the cleanest illustration in the
set of why the kernel has a price-observed correction.

This does not show that the counter is broken. It may lag beyond the window
watched, or count on a different boundary. What is recorded is what was
observed: three reads, no movement.

## No licence metadata

No publisher, author, rights, terms or licence field on any response. The
licence column is unknown for Tavily, entirely.

## Fields present but unpopulated

`raw_content` is `null` on every result unless `include_raw_content` is set;
`answer` and `follow_up_questions` are `null` unless requested; `images` is an
empty array. Present is not populated, and the envelope should record these as
unobserved rather than absent.

## Useful for the envelope

`request_id` (UUID, a real provider request identifier), `response_time`
(**seconds** as a float, not milliseconds — the one unit trap in this provider),
`score` (provider-native relevance, must stay namespaced per the provider
contract rather than normalised against Exa's or Firecrawl's ordering).

## Extract, 5 September 2026

One call with `TAVILY_API_KEY` to `POST /extract` of
`https://www.w3.org/TR/prov-dm/`: `200`, 808 ms, one result with the page in
`raw_content`, `failed_results` empty, `request_id` returned
(`tavily-extract.json`). No cost field; the published rate gives no
single-URL figure, so the charge is recorded unknown.
