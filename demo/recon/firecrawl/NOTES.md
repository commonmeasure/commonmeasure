# Firecrawl reconnaissance notes, 1 August 2026

Probed 1 August 2026. Two requests, 3 credits, no currency reported.

## The charge moves between endpoints

`creditsUsed` is reported on both operations, but **not in the same place**:

| Operation | Path to the charge | Value |
|---|---|---|
| `POST /v2/search`, limit 2 | `body.creditsUsed` (top level) | 2 |
| `POST /v2/scrape`, markdown | `body.data.metadata.creditsUsed` (nested) | 1 |

Firecrawl's API reference documents the top-level field for search and does not
mention the scrape location. An adapter that reads `body.creditsUsed` uniformly
gets `2` for search and `None` for scrape, and silently records a fetch as
having cost nothing. A first reading of the response made exactly this
mistake; dumping the full key list caught it.

Search cost 1 credit per result returned, which is why limit 2 cost 2 credits.

## Credits, not currency

There is no USD figure anywhere in the response. Converting credits to money
depends on the account plan, which the API does not state. The envelope should
carry `2 credits` with the billing unit named, not a dollar figure derived from
a plan assumption. Exa is the only provider here that gives currency directly.

## It declares its own cache state, and charges anyway

The scrape response metadata included:

```json
"cacheState": "hit", "cachedAt": "2026-08-01T03:18:25.618Z", "proxyUsed": "basic"
```

Firecrawl is the only provider in the set that **tells you** it served a cached
copy. It still charged 1 credit, and it returned in 611 ms.

This is genuinely useful for the demo rather than merely a caveat: a fair
latency comparison needs to know which responses were cold, and Firecrawl is
the only provider that says so. Exa served a cached copy too but only disclosed
it as `source: "cached"` on `/contents`; Tavily discloses nothing.

## No licence metadata, despite rich metadata

The scrape returns a large metadata object — `og:title`, `og:description`,
`og:type`, `og:site_name`, `ogLocaleAlternate` (23 locales), `Generator`,
`language`, `favicon`, `sourceURL`, `statusCode`, `contentType`. Not one field
concerns rights, licence, terms or permission.

`og:site_name` is the closest thing to a publisher name, and it is a
publisher-authored SEO tag, not a rights statement. It must not be promoted
into the licence column. This is precisely the "never convert a missing field
into a permission" case: the object looks rich enough that it is tempting to
read consent into it.

## Useful for the envelope

`id` (search request identifier, UUIDv7), `scrapeId` (scrape identifier),
`position` (native rank), `statusCode`, `contentType`, `sourceURL` (canonical
URL after redirects), `X-Response-Time` response header.
