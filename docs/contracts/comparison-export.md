---
title: Comparison results export
---

# Comparison results export

The Edge console's **Export results** action reads a retained retrieval
comparison and downloads a ZIP. It makes no supplier or model call, requires
no Hub connection and does not alter the source record. An operator can share
the downloaded file through their usual channels. The file is a static copy;
it can be forwarded and cannot be revoked by deleting the original.

Open a retained comparison, choose whether to **Include query text**, then
select **Export results**. Query text is excluded by default. Internal-source
comparisons cannot be exported in this first version, including remote-provider
records with internal crossings, withheld private result metadata or withheld
refusal details. **Download private source record** is a separate local JSON
download containing private evidence; use the ZIP for sharing.

## Contents and disclosure

The version is `contextops-comparison-export/v1`, independent of the private
`contextops-comparison/1` record. The archive contains exactly:

- `report.html`: self-contained readable report, with no network assets or
  scripts. It can be opened without a Common Measure account.
- `summary.csv`: provider/metric rows with values, units, measurement basis,
  missing reasons and case/operation counts.
- `cases.csv`: the same measurements with case and attempt identity and the
  optional query. This first version contains one retrieval case, `case-1`.
- `manifest.json`: the typed projected result, format/producer versions and
  SHA-256 digests and lengths of the other three files. It carries no digest
  of itself. These digests detect changes; the archive is not signed.

Only comparison UUID, normalised timestamps, requested/effective result limits,
policy mode, provider and adapter version, operation outcomes, recorded counts,
charge amounts/units/basis and timing are projected. With query inclusion, the
original query also appears in the report, case table and manifest. Query text
is copied exactly and may itself contain private information; the operator
must review it before sharing. The preview beside the export control states
these fields before download.

The projection excludes fields for query hashes, source URLs/titles/passages,
answers, raw responses, supplier request IDs, endpoints, principal identifiers, local paths,
policy details, charge notes, refusal details and free-text errors. Future
fields in local evidence do not automatically enter the export. HTML escapes
all values; CSV quotes cells and prefixes potential spreadsheet formulas with
an apostrophe. The JSON query retains its exact text.

## Meaning and incomplete results

This is a retrieval probe, not an answer-quality or document-relevance
benchmark. Those evaluations remain unavailable; no score is invented.

Missing numbers are JSON null or empty CSV numeric cells, with a missing
reason; the report displays Unknown. Recorded zero remains zero. Observed
money and native-unit charges are separate measurements; a quoted charge stays
quoted. Costs are not combined across currencies or units, and unknown cost
is never counted as zero. The CSV `unknown_cost_operations` counts selected
provider operations with no recorded price, including operations that did not
start; it is not a count of billed API calls.

A provider operation is `completed`, `refused` or `unavailable` only when its
terminal record says so. A started operation without a terminal record is
`outcome_unknown`; a selected provider without a start is `not_started`, with
no attempt number. Starting an operation does not prove an HTTP request was
sent. The comparison is complete only when its completion record exists and
all selected providers have terminal records. Completed comparisons can still
contain refusals or unavailable providers. Evidence-write and allowance-accounting
gap counts are preserved in every output, without their private details; a
terminal provider outcome does not establish complete evidence or accounting.

Unknown format versions, internal comparisons, invalid identities and
inconsistent provider sequences are refused. Export never repairs, retries or
re-executes a comparison. Incomplete evidence can be exported as an explicitly
incomplete report, preserving possible-charge uncertainty.

## Console endpoint and ownership

`GET /api/compare/export?comparison=<id>` downloads the metrics-only ZIP;
`include_query=true` opts into query inclusion. An invalid query option gets
400, a missing/unreadable comparison gets 404, and unsupported export evidence
gets 422. Responses have `Content-Type: application/zip`, an attachment filename
and `Cache-Control: no-store`. Existing loopback Host protection applies.

Projection is owned by `crates/commonmeasure-harness/src/compare_export.rs`;
archive, CSV and report rendering by
`crates/commonmeasure-console/src/compare_export.rs`. Both consume retained
comparison evidence. Hub authentication, recipient accounts and hosted share
links are not part of this local endpoint. Future Hub export creation must
apply the chosen admin role and tenant checks separately.

Benchmark dataset, model and evaluator metadata and multi-case aggregation
are proposed and not part of this export.
