---
title: People Inc content
domain: extensions
audience: operator
section: integrate
---

# People Inc content

The built-in `peopleinc` adapter searches the People Inc Content API and
retrieves full article markdown. Search results already include full markdown;
Common Measure does not make a second retrieval to obtain it.

## Configuration

Set `PEOPLEINC_API_KEY` in the environment that starts Common Measure, in the
operator's `credentials.env`, or through the console's Sources page. See
[Credentials](../OPERATING.md#credentials-when-you-want-provider-search) for
precedence, file permissions and when running sessions pick up a saved key.
The key is sent only in the `X-API-Key` request header. A missing key produces
an unavailable result naming the variable; there is no anonymous fallback.

With an issued key and authority for any charges, call `context_search` with
`provider: "peopleinc"`, a `query` and a `limit`. The adapter sends
`POST /v1/search` with a trimmed `question`, a limit clamped to 1–5 and
`strategy: "balanced"`. Empty questions and questions longer than 2000 Unicode
characters after trimming are refused before a request. A batch job requesting
more than five results records a coverage gap naming the provider's ceiling.

People Inc documents no host filtering parameter. Common Measure checks every
returned source URL against the source policy; the supplier's `domain` label
(for example, `Lifestyle`) is retained as metadata and is never used as a
policy host. Search returns documents in the supplier's order, with their full
markdown, stable identifier, rank and metadata. The selection summary stays in
the sealed raw response. Publication dates carry supplier provenance;
`metadata.update` supplies a declared date only when `pubdate` is absent.
Both fields remain in native metadata. Missing metadata remains missing.

## Named article retrieval

The batch runner dispatches `fetch` when a job names a `fetch_target` and
includes `peopleinc` in its providers. The adapter sends
`GET /v1/markdown` with the exact target as a standard URL-encoded query value.
This route is covered by synthetic loopback tests through planning, transport,
admission and source-record sealing. It is **not** selectable through
`context_fetch`, which still uses the direct governed fetch path.

Use a fully-qualified HTTPS target without a query, fragment, credentials or
non-default port. Invalid targets are refused rather than changed to a
supposed canonical article. The supplier guide asks for no trailing slash but
shows search results with trailing slashes and describes them as suitable for
retrieval. Common Measure accepts those slashes unchanged. Their acceptance by
the live supplier remains unverified; it does not strip a slash to substitute
another identity.

## Rights, billing and failures

A description of a licensed corpus does not provide a machine-readable grant.
Licence/agreement and monetary cost remain unknown. Every HTTP 200 from the
markdown endpoint is billable; search pricing depends on the licensing
agreement. Common Measure declares no price and makes no free-call claim.

Each operation sends one request. This slice has **no automatic retry policy**:
4xx and 5xx failures return to the caller, with an attempt count of one. The
error retains the RFC 9457 Problem Details object, validation errors and
optional `requestId`. A 429 also retains `Retry-After`; wait that period before
an explicitly authorised retry. No successful paid call is automatically
repeated. Any future automatic 5xx retry policy needs bounded exponential
backoff with jitter, starting at one second and capped at 30 seconds, and
source-record evidence of each attempt.

A 404 `redirectUrl` or an HTTP redirect is a candidate requiring confirmation,
not authority to retrieve a replacement. Neither is followed automatically.
A 403 returns the supplier's refusal and never triggers an unlicensed fallback.
Missing, malformed or empty markdown produces an explicit failure, not a
successful empty article. An empty search `documents` array is a valid result.

There is no authorised body cache in this slice. ETags are not used and
`If-None-Match` is never sent. Repeated fetches are unconditional and each
successful response is billable. An unsolicited 304 fails explicitly because
there is no matching authorised cached body to return. Successful response
bytes are sealed unchanged; a response that echoes the credential is refused
rather than sealed. Error and transport diagnostics redact credential echoes.

## Verification

Search and fetch are `fixture-tested` and `spec-verified`, not `live-verified`
or `replay-tested`. Synthetic fixtures contain no licensed article content.
The primary integration guide was retrieved through Common Measure on
7 October 2026:

- [People Inc Content API v1 integration guide](https://contentmarketplace.people.inc/documentation/v1/integration-guide.html)
- Source text hash: `sha256:9813ce789e2f91f5a68a154963a7fc7338b6109ebff0aa13b5e65a62929b1c38`.
- Adapter fixtures: `crates/commonmeasure-supply/tests/peopleinc_spec.rs`.
- Governed batch-path fixtures: `crates/commonmeasure-runtime/tests/peopleinc_supply.rs`.

No supplier key or live billing authorisation establishes these states.
The [provider contract](../contracts/provider.md) owns the capability and
verification inventory.
