# Changelog

Each release's section is also its GitHub release notes: the release
workflow publishes the section whose heading names the tag's version.
Versions follow [Semantic Versioning](https://semver.org/); before 1.0.0 a
minor version may break compatibility.

## 0.4.0 (23 September 2026)

`robots.txt` now binds in every policy mode. An operator who ran `observe`
or `prefer` and relied on the edge fetching a disallowed page will see that
crossing refused.

### robots.txt

- A `Disallow` addressed to `CommonMeasureBot`, or to `*` where no group
  names it, refuses the crossing before the page is fetched in every policy
  mode: `strict`, `observe` and `prefer` alike. In earlier releases
  `observe` and `prefer` fetched the page and recorded the rule.
- A `robots.txt` that cannot be read follows RFC 9309 §2.3.1 in every mode.
  A 4xx other than 429 means no rules. A 429, a 5xx, a timeout, or a name,
  TLS or connection failure means unreachable: the last answer the host
  gave rules, however old; with none held, the crossing is refused as a
  complete disallow. A failed probe never replaces the last answer held.
- A `robots.txt` larger than 512 KiB is parsed up to its last complete line
  within that bound (RFC 9309 §2.5), and the record carries `truncated` with
  the file's size and the bytes read. Earlier releases discarded an
  oversized file and fetched with no rules, so padding the file switched its
  `Disallow` off.
- A `robots.txt` redirect the edge declines to follow (to an address it does
  not mediate, to the hub's origin, or to a host the operator's policy
  refuses) counts as unreachable. More than five redirects is unreachable.
- A robots refusal names the source's `robots.txt`, not the operator's
  policy file. The record carries the address of the file that ruled, so a
  redirected `robots.txt` is named by where it was read from.
- A `robots.txt` request that the edge cut short (the call's time limit left
  no time for it, or it could not be signed) refuses the crossing in every
  mode and is not held as the host's failure.

## Earlier releases

- **0.3.5** (22 September 2026): registered network instances with a
  delivery duty, hub-released supplier credentials, a hosted MCP transport,
  Agents and Compare views in the console, and a publisher's reporting
  demand, `Crawl-delay` and licence honoured before a page is read.
- **0.3.4** (16 September 2026): `commonmeasure disconnect` works against a
  hosted hub.
- **0.3.3** (15 September 2026): project-directory reporting consent, and
  source record, budget and policy views in the console.
- **0.3.2** (14 September 2026): managed-policy synchronisation and
  session-file path fixes, and compatibility with more agent hosts.
- **0.3.1** (14 September 2026): three more hosts registered by the binary,
  one-command hub enrolment for a managed machine, and strict policy's
  handling of personal data found on a public page.
- **0.3.0** (13 September 2026): the first public release, downloadable and
  usable without an account.
- **0.2.0** (6 September 2026): tagged before the first public release.
