# Keenable re-verification notes, 5 September 2026

One call with a `keen_`-prefixed `KEENABLE_API_KEY` (`X-API-Key`), `200`,
492 ms: `POST /v1/search` with `snippet_max_length: 3000` and the
single-domain `site: www.w3.org` filter. Ten results in
`results[{title, url, description, snippet, acquired_at}]`; text is taken
from `snippet`, `description` arrives empty. The `site` filter matched
`dvcs.w3.org` URLs, which the job's source policy then refused at admission,
so the run admitted nothing from this plan. No cost field and no published
flat price: the charge is recorded unknown.
