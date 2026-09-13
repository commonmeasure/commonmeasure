# Exa reconnaissance notes, 1 August 2026

Probed 1 August 2026. Two requests, USD 0.008 observed in total.

## Cost is reported in dollars, and the total is trustworthy

`costDollars.total`, a decimal USD float, on both endpoints. Exa is the only
provider in the set that reports acquisition cost in currency rather than
credits, and the only one whose observed charge could be compared against the
published price at all:

| Operation | Observed `total` | Published | Match |
|---|---|---|---|
| `POST /search`, 2 results + text | 0.007 | USD 7 / 1k requests | exact |
| `POST /contents`, 1 URL, text | 0.001 | USD 1 / 1k pages | exact |

## Use the total; treat the breakdown as indicative

On the combined search-plus-contents call, `costDollars` was:

```json
{ "total": 0.007, "search": { "neural": 0.007 } }
```

There is **no `contents` key**, even though text was returned for both results.
The isolated contents call did itemise it (`"contents": {"text": 0.001}`).

So on a combined call the whole charge is attributed to `search.neural`, and an
adapter that reads `costDollars.contents` to separate extraction cost from
search cost records **zero**. Read `costDollars.total` and treat the breakdown
as indicative only.

Note also what the total implies: two pages of text arrived without adding
USD 0.002 to a USD 0.007 search. Whether bundled contents is genuinely free or
merely unbilled on this call is not settled by one observation, and neither
is claimed here.

## Licence metadata is absent

`author` exists on `/contents` results. It came back `null`. There is no
publisher, rights, terms or licence field on any Exa response. Per
`docs/contracts/experiment.md`, a null `author` is unknown, not unattributed.

## Cached delivery: charge and latency

`/contents` returned `statuses: [{"status": "success", "source": "cached"}]` in
**217 ms** and charged the full USD 0.001. That latency is a cache-hit latency,
not an acquisition latency. Comparing it against another provider's cold fetch
would flatter Exa for reasons that have nothing to do with the provider.

## Unexplained payment headers

Response headers include:

```
access-control-expose-headers: PAYMENT-REQUIRED,PAYMENT-RESPONSE,
  payment-required,payment-response,WWW-Authenticate,Payment-Receipt
```

A `Payment-Receipt` header is exactly the shape the `corroborate` capability
wants. None of these headers were present on the actual responses, only
declared as exposable, and `exa.ai/docs/reference/x402` returns 404. Recorded as
an observation with an unverified purpose, not as a capability.

## Useful for the envelope

`requestId` (provider request identifier), `searchTime` (ms, float), `title`,
`url`, `id` (canonical URL), `image`, `favicon`.
