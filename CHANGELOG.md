# Changelog

Each release's section is also its GitHub release notes: the release
workflow publishes the section whose heading names the tag's version.
Versions follow [Semantic Versioning](https://semver.org/); before 1.0.0 a
minor version may break compatibility. `RELEASING.md` §Release notes says how
a section is written.

## 0.4.1 (25 September 2026)

**Upgrading:** 0.4.1 upgrades from 0.4.0 only. It refuses a relay spool that
holds `relay/spool/outbound.ack` or queue lines without an index, which 0.3.4
and earlier wrote, and sends nothing from it. On such a home:

1. Stop every relay, including `commonmeasure hosted service`
   ([one release per Edge home](https://github.com/commonmeasure/commonmeasure/blob/main/ARCHITECTURE.md#what-runs-where)).
2. Upgrade.
3. Run `commonmeasure status` and read its `refused spool` line
   (`egress.refused_spool` with `--json`): how many indexed batches are still
   queued or dead, and whether that count is complete.
4. Move `relay/spool` aside, keeping it and `relay/delivered.idx`.
5. Run `commonmeasure relay` without `--session`, or restart the hosted
   service, whose interval run does the same. It projects outstanding events
   again from every session log under `sessions/`, under the approvals and
   policy in force, and queues no event that `relay/delivered.idx` records as
   accepted. The one exception is an event resent under its own id to carry a
   session's changed refused count. Pass `--run` again for each published run
   whose events were outstanding.

An outstanding event whose session log or run input is gone is not projected
again: it stays in the saved spool, undelivered
([delivery state](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/telemetry-projection.md#delivery-state)).

### Added

- Reported telemetry now includes `data.commonmeasure-crawl-delay` on mediated retrievals with a recorded, honoured `Crawl-delay` above zero, capped at 60 seconds; it is absent when that evidence is missing ([telemetry projection](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/telemetry-projection.md)).

### Changed

- The edge reports its release with every directory proof, and the hub leaves a key unlisted when the last accepted upload reported no release or one below its floor. Upgrade the edge to at least the hub's floor: the key stays unlisted until that edge uploads a proof. After an upgrade, the edge uploads on its next relay run; a failed upload due only to the release change waits an hour before another attempt ([enrolment](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/enrolment.md)).
- Failing hosts put later mediated requests into back-off, shared across sessions and restarts on one edge: 429 and 503 honour a usable `Retry-After` up to one hour; other 5xx responses, and 429 or 503 without a usable header, start at 10 seconds and double for consecutive failure periods up to 15 minutes. Failed requests are never resent automatically; later requests wait within their remaining budget or are refused, and one sender goes first after back-off ends ([failure policy](https://github.com/commonmeasure/commonmeasure/blob/main/docs/FAIL-POLICY.md)).
- Hosted back-off refusals say only that the host is in back-off, except that they may give the end when the host supplied it as an uncapped `Retry-After` HTTP-date; other tenants' response status, failure count and timing remain private ([session evidence](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/session-evidence.md)).

### Removed

- The relay no longer imports acknowledgements from `outbound.ack` or reads
  a queue line by its position. It refuses such a spool and names the file
  or line and the remedy; `status` and `doctor` say how many batches in its
  indexed lines are still queued or dead, and whether that count is complete
  ([delivery state](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/telemetry-projection.md#delivery-state)).
- A queued batch without `directory_selection` is no longer sent as if no
  directory selection applied: the line is reported as damaged. An
  undelivered batch that 0.3.4 or earlier queued is held and never sent
  ([directory enrolment](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/directory-enrolment.md)).
- `doctor` no longer reads a `relay/receipts.json` that names no accepting
  receiver as a delivery to the configured one; it says the file is from
  0.3.4 or earlier and that the next delivery rewrites it.
- The fleet-status classifier no longer compares an edge that reports no
  envelope digest (0.3.0) on its loader digest; it reads as `unknown`
  ([fleet status](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/fleet-status.md#what-a-receiver-concludes)).

### Fixed

- Directory-proof failures retain non-JSON error bodies up to 2 KiB on a character boundary, with a truncation marker giving the original byte count; JSON `detail` and `message` strings remain whole ([enrolment](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/enrolment.md)).
- Telemetry receivers can acknowledge a batch with any `2xx` response, including an empty body; when the response supplies no valid count of newly recorded events, the relay reports that count as unknown ([receiving reports](https://github.com/commonmeasure/commonmeasure/blob/main/docs/integrate/reports.md)).
- A relay run uses the reporting approvals it has just refreshed, so an approval renewed after expiry allows reporting during that run without a separate enrolment sync ([telemetry projection](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/telemetry-projection.md)).

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
