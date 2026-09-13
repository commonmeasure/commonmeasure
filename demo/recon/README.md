# Recorded provider responses

Retained evidence of dated live calls, unchanged since capture. Two
generations are here: the provider reconnaissance of 1 August 2026 (Exa,
Firecrawl, Tavily search, TollBit) and 4 August 2026 (Redpine, the first
licensed fetch), and the re-verification captures of 5 September 2026
(Parallel search and extract, Linkup search and fetch, Keenable, You.com and
Nimble search, Tavily extract).

These bytes are served from a real loopback origin to the real provider
adapters in `crates/commonmeasure-supply/tests/recorded_replay.rs` and
`crates/commonmeasure-runtime/tests/end_to_end.rs`: the client, the framing, the
parser, the policy and the evidence log under test are the ones a live run
uses, and only the origin is local. The `--replay` run mode
(`docs/contracts/run-output.md`) enters through the same boundary:
`replay-manifest.json` in this directory names, for each provider and
capability, which capture a replay run serves, its capture date, its
redactions, its permitted use and the SHA-256 of the served bytes. The runtime
refuses to serve a capture whose bytes no longer match the manifest.

`fixture-manifest.json` names the captures on the other side of that line:
the ones an adapter test serves over loopback and no run serves. It has the
same shape and the same hash rule, so every capture put on the wire anywhere
in this repository has a declared hash, and an altered capture is refused on
both paths.

## What is in each file

One JSON object per call, holding the request that was made and the response
that came back:

- `request.headers` — credential values are replaced with
  `«REDACTED: value of $VAR, never recorded»`. The redaction is deliberately
  visible so that a reader can see which variable authenticated the call
  without the value existing in the repository. No credential value was
  written to disk at any point.
- `response.status`, `response.latency_ms`, `response.body` — as returned.
  The August captures also keep `response.headers`, because two findings come
  from them (Exa's payment headers, Firecrawl's cache declaration). The
  September captures were made by `commonmeasure run --live`, which seals the
  body, the status and the latency and does not record response headers.

## Elision

Every page-text field (`text`, `content`, `raw_content`, `markdown`,
`description`, `body`, `excerpts`, `full_content`, `snippet`, `snippets`) is
truncated to a 120-character lead followed by
`[…elided, N chars total; shape recorded, text not committed]`. The field, its
type and its true length survive; the body text does not. An adapter author
can see the shape without the repository holding page bodies.

No TollBit response contained body text: its search returns no excerpt, and
no licensed fetch succeeded there.

## Reproducing

The September captures come from two committed jobs, each run live with the
provider credentials in the environment and then replayed from this directory:

```
commonmeasure run demo/jobs/recon-w3c-prov-search.json --live --output <dir>
commonmeasure run demo/jobs/recon-w3c-prov-fetch.json --live --output <dir>
commonmeasure run demo/jobs/recon-w3c-prov-search.json --replay demo/recon --output <dir>
commonmeasure run demo/jobs/recon-w3c-prov-fetch.json --replay demo/recon --output <dir>
```

A live run seals each provider's response under `responses/`; the capture
file is that body, elided as above, with the request the adapter constructs
(`crates/commonmeasure-supply/src/<provider>.rs`). The August captures record the exact
method, URL and body of each probe, so any of them can be reissued by hand.

## Findings

Per-provider notes are in `<provider>/NOTES.md`. The verification states and
the list of what could not be verified are in
`docs/knowledge-base/provider-verification.md`.
