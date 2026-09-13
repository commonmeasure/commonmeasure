# Nimble re-verification notes, 5 September 2026

Two calls with `NIMBLE_API_KEY` (`Authorization: Bearer`) to `POST /v2/search`
with `search_depth: fast`, `max_results: 3`, `include_domains: ["www.w3.org"]`:

- the full PROV-DM question did not answer within the runtime's 30 s exchange
  budget and was recorded as a transport failure, no bytes;
- the shorter query `W3C PROV data model PROV-DM Recommendation` answered
  `200` in 1,128 ms with three results (`nimble-search.json`).

No cost, credit or usage field: the charge is quoted from the published
fast-depth price of 2 credits.
