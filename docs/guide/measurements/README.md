---
title: Measurements behind the guide
---

# Measurements behind the guide

Three measurements support the guide: token density by file type, the cost of
raw HTML, and overlap between search providers. The repository can rerun the
file-density measurement and the source-overlap analysis. The
raw-HTML figures cannot be reproduced exactly because their original inputs
are not retained; the script can only repeat the method on newly saved pages.

The two scripts need Python 3 and a virtual environment; neither needs the
Common Measure binary:

```sh
python3 -m venv .venv
.venv/bin/pip install tiktoken markdown
```

## `token_density.py`

How much of a context window different kinds of content cost, and how much of
a raw web page is markup rather than text.

```sh
.venv/bin/python docs/guide/measurements/token_density.py
```

Reads committed files from this repository — prose, Rust, JSON Schema, an
event log, `Cargo.lock` — and reports tokens per 1,000 characters for each.
Tokeniser is tiktoken `o200k_base`, named because counts do not transfer
between vendors; Anthropic and Google publish no downloadable tokeniser, so
only the ratios between rows carry across.

To repeat the raw-HTML method in the guide's chapter 1, put saved pages
in a `pages/` directory beside the script as `*.html`. The four used were an
arXiv abstract, a Wikipedia article, a vLLM documentation page and the BBC News
index, fetched 4 August 2026. They are not committed, because they are
third-party content and they go stale. A new run therefore repeats the method
on whatever you fetch; it does not re-derive the published figures.

`html-token-density-2026-08-04.json` is committed and records, for each of the
four pages, the URL, fetch date, byte size, raw and stripped token counts, and
a SHA-256 of the exact bytes measured. That does not let you reproduce the
numbers without the original pages, but it pins what was measured, and anyone
refetching a URL can tell whether the page has changed since.

Tag stripping is a deliberately crude regular expression, not an extractor. A
real extractor also drops navigation and footers, so the reported saving is a
floor.

## `source_overlap.py`

How much three open-web search providers agree with each other, and what a
second provider adds.

```sh
.venv/bin/python docs/guide/measurements/source_overlap.py --pilot   # 1 query
.venv/bin/python docs/guide/measurements/source_overlap.py --out results.json
```

Twenty-four frozen queries across six kinds, ten results each. Request shapes
follow the live-verified adapters in `crates/commonmeasure-supply/`, so this exercises
the same provider behaviour the product sees.

**It spends money.** The full run is 72 searches. It is search-only — no
`contents` on Exa, no `scrapeOptions` on Firecrawl — so no page-content or
scrape charges are incurred, but the searches themselves are billed. Run
`--pilot` first.

Credentials come from the environment or from `.env` at the repository root.
A provider without a key is reported as unavailable and excluded; a failed
request is recorded as an error and excluded from the statistics rather than
counted as an empty result set.

Results are compared by host rather than URL, because two providers returning
different pages from the same site are not independent evidence. Host is the
hostname minus a leading `www.`, not a public-suffix parse.

`source-overlap-2026-08-04.json` holds the run quoted in the guide, including
the per-query results and the four failed calls.
