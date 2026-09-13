# Parallel re-verification notes, 5 September 2026

Two calls with `PARALLEL_API_KEY`, both `200`:

- `parallel-search.json`: `POST /v1/search`, the PROV-DM question scoped to
  `www.w3.org` through `advanced_settings.source_policy.include_domains`.
  Ten results in `results[{url, title, publish_date, excerpts}]`; the job
  admitted three. `usage` = one `sku_search`. 2,890 ms.
- `parallel-extract.json`: `POST /v1/extract` of `https://www.w3.org/TR/prov-dm/`
  with `full_content: true`. One result carrying `full_content` and
  `excerpts`; a `warnings` entry notes that neither `objective` nor
  `search_queries` was sent, which the extract path does not need.
  `usage` = one `sku_extract_excerpts`. 404 ms.

Parallel reports usage as billing SKUs and discloses no currency, so the
charge is observed in SKUs and a USD cap cannot be enforced against it.
