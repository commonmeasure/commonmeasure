# Linkup re-verification notes, 5 September 2026

Two calls with `LINKUP_API_KEY` (`Authorization: Bearer`), both `200`:

- `linkup-search.json`: `POST /v1/search`, `depth: standard`,
  `outputType: searchResults`, `maxResults: 3`, `includeDomains: ["www.w3.org"]`.
  Three results in `results[{type, name, url, content, favicon}]`. 3,459 ms.
- `linkup-fetch.json`: `POST /v1/fetch` of `https://www.w3.org/TR/prov-dm/`.
  The page arrives as `markdown` with a `favicon`. 508 ms.

Neither response carries a cost field. The search charge is quoted from the
published standard-depth price; the fetch charge is unknown.
