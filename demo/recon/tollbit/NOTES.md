# TollBit reconnaissance notes, 1 August 2026

Probed 1 August 2026. Six requests, no charge reported on any of them, no
licensed fetch achieved.

## The developer key authenticates

No TollBit request had carried a real key before this probe.
`GET /dev/v2/search` answered **200** on the first
attempt with the `TollbitKey` header. Assumption U2 (key travels in `TollbitKey`)
moves from spec-verified to live-verified, and U7 (search exists) from
"exists, returns 401" to "exists, returns results".


## It is the only provider in the set that returns provenance

TollBit search results carry `publisher {domain, name}`, `publishedDate` and
`availability {discoverable, readyToLicense}`. Exa, Tavily and Firecrawl return
nothing of the kind on any endpoint tested. If the dashboard's
licence/provenance column has any populated cell at all, it comes from here.

## `readyToLicense` did not survive contact with the rates endpoint

This is the finding most likely to break adapter work.

A 20-result search returned 2 items with `readyToLicense: true`
(`bakercityherald.com`, `zmescience.com`) and 18 with `false`. Every premium
publisher in the page — CNN, NPR, Reuters, The Atlantic, AP, PBS, Pew — came
back `discoverable: true, readyToLicense: false`.

`GET /dev/v2/rates/{url}` was then called on three of them:

| URL | `readyToLicense` | rates |
|---|---|---|
| bakercityherald.com | **true** | 403 |
| zmescience.com | **true** | 403 |
| npr.org | false | 403 |

All three returned an identical `application/problem+json` body: *"The content
provider has disallowed access to this page."*

So `availability.readyToLicense` is **not** a licensability signal, at least on
this account. The adapter filters search candidates on that flag
precisely to avoid "a path that can only fail at `rate`" — and on this key the
surviving candidates fail at `rate` anyway. Filtering on it discards 18 of 20
results and buys nothing.

Whether the 403 is per-page or account-wide (a key with no publisher
agreements) cannot be told apart from three probes returning the same generic
message. No further spend was made to find out, because either way no price
and no licence reference is obtainable today.

## Token creation returns 500

`POST /dev/v2/tokens/content` with `maxPriceMicros: 10000` (USD 0.01, inside
the per-request cap) returned **500 Internal Server Error** with no `detail`
field — unlike the rates 403, which was a well-formed problem document. No
content fetch was attempted afterwards. Nothing was purchased.

## Consequences for the adapter

- Search is usable now and returns real publisher identity.
- Rate, token and content remain **unverified**; no TollBit price has ever been
  observed, so the fixture micro-prices stay fixtures.
- Search carries no request identifier in body or headers, so TollBit is the one
  provider with no join key for a provider-side receipt.

## 4 August 2026 — the demo publisher is disallowed too

The open question above — per-page or account-wide — is answered. TollBit's
quickstart walks a token purchase for
`https://tollbit.pioneervalleygazette.com/daydream`, an obviously fictional
demo publisher. `GET /dev/v2/rates/` on that URL, probed both in the recon
convention (full URL, percent-encoded) and in the quickstart's own bare form,
returns the same 403 problem document as bakercityherald, zmescience and npr:
"The content provider has disallowed access to this page"
(`tollbit-rates-demo-publisher.json`).

A key for which even the documentation's demo target is disallowed is blocked
account-wide, not by publishers. The quickstart names a prerequisite no probe
here has exercised: an AgentID registered in the developer dashboard, whose
user agent the token request must carry. Whether registration alone lifts the
403, or funded billing is also needed (U14), is settled in the dashboard, not
from here. Nothing was purchased; rates lookups reported no charge, as on
1 August.

## 4 August 2026, later — AgentID registration changes nothing observable

An AgentID, `common-measure-tbcli-catb`, was registered in the developer dashboard
and the probes were repeated with its example user agent,
`common-measure-tbcli-catb/1.0 (compatible; +https://tollbit.com)`
(`tollbit-agent-probes.json`). Rates on the demo publisher: 403, both URL
forms. Rates on bakercityherald: 403. Token creation for the demo publisher:
the same bodyless 500, tried against `/dev/v2/tokens/content`, the
quickstart's `/tollbit/dev/v2/tokens/content`, and with the full subdomain
URL in the body.

A registered agent requesting the documentation's own demo target, refused at
quote and erroring at purchase, is a supplier-side fault or an account state
the API does not name — U14's funded-billing question remains the one lever
left. Next: the dashboard's payment settings, then TollBit support with these
timestamps.
