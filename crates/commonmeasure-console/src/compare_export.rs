//! Offline comparison reports. Every output consumes the same typed projection.

use std::io::{Cursor, Write};

use commonmeasure_harness::compare_export::{Export, project};
use commonmeasure_types::canonical::sha256_digest;
use maud::{DOCTYPE, html};
use serde_json::{Value, json};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

/// Make a portable archive without contacting a supplier or inspecting policy.
/// The caller passes an already retained comparison, not a live search result.
pub fn archive(record: &Value, include_query: bool) -> Result<Vec<u8>, String> {
    let export = project(record, include_query)?;
    let mut files = vec![
        ("report.html", report(&export).into_bytes()),
        ("summary.csv", csv(&export, false).into_bytes()),
        ("cases.csv", csv(&export, true).into_bytes()),
    ];
    let hashes: Vec<_> = files
        .iter()
        .map(|(name, bytes)| {
            json!({
                "name": name, "bytes": bytes.len(), "sha256": sha256_digest(bytes)
            })
        })
        .collect();
    let manifest = json!({
        "schema": export.schema,
        "result": export,
        "files": hashes,
        "integrity": "File digests detect changes; this archive is not signed.",
        "limitations": [
            "Retrieval probe only: answer correctness and document relevance were not evaluated.",
            "Missing cost remains unknown. Quoted cost is not an observed charge.",
            "A started operation without a result may have incurred a charge.",
            "This static export can be forwarded and cannot be revoked."
        ]
    });
    files.push((
        "manifest.json",
        serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
    ));
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, bytes) in files {
        zip.start_file(name, options).map_err(|e| e.to_string())?;
        zip.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    Ok(zip.finish().map_err(|e| e.to_string())?.into_inner())
}

fn csv(export: &Export, cases: bool) -> String {
    let mut out = String::new();
    let mut header = vec![
        "provider",
        "outcome",
        "metric",
        "value",
        "unit",
        "basis",
        "missing_reason",
    ];
    if cases {
        header.extend(["case_id", "attempt", "query"]);
    } else {
        header.extend([
            "selected_cases",
            "finished_operations",
            "unknown_cost_operations",
        ]);
    }
    header.extend(["evidence_gap_count", "allowance_gap_count"]);
    let evidence_gaps = export.evidence_gap_count.to_string();
    let allowance_gaps = export.allowance_gap_count.to_string();
    csv_row(&mut out, &header);
    for provider in &export.providers {
        let unknown_cost = provider
            .metrics
            .iter()
            .any(|m| m.name == "acquisition_cost" && m.value.is_none());
        for metric in &provider.metrics {
            let value = metric
                .value
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default();
            let attempt = provider.attempt.map(|n| n.to_string()).unwrap_or_default();
            let mut row = vec![
                provider.provider.as_str(),
                provider.outcome,
                metric.name,
                &value,
                &metric.unit,
                metric.basis,
                metric.missing_reason.unwrap_or(""),
            ];
            if cases {
                row.extend(["case-1", &attempt, export.query.as_deref().unwrap_or("")]);
            } else {
                row.extend([
                    "1",
                    if matches!(provider.outcome, "completed" | "refused" | "unavailable") {
                        "1"
                    } else {
                        "0"
                    },
                    if unknown_cost { "1" } else { "0" },
                ]);
            }
            row.extend([evidence_gaps.as_str(), allowance_gaps.as_str()]);
            csv_row(&mut out, &row);
        }
    }
    out
}

// Quoting protects commas/newlines; an apostrophe also stops spreadsheet formula
// execution, which CSV quoting alone does not prevent. JSON retains exact text.
fn csv_row(out: &mut String, fields: &[&str]) {
    for (index, field) in fields.iter().enumerate() {
        if index != 0 {
            out.push(',');
        }
        out.push('"');
        if field.trim_start().starts_with(['=', '+', '-', '@'])
            || field.starts_with(['\t', '\r', '\n'])
        {
            out.push('\'');
        }
        out.push_str(&field.replace('"', "\"\""));
        out.push('"');
    }
    out.push_str("\r\n");
}

fn metric_label(name: &str) -> &str {
    match name {
        "received" => "Received results",
        "admitted" => "Admitted results",
        "refused" => "Refused results",
        "request_latency" => "Request time",
        "elapsed" => "Total time",
        "acquisition_cost" => "Acquisition cost",
        "acquisition_cost_native" => "Native-unit cost",
        _ => name,
    }
}

fn outcome_label(outcome: &str) -> &str {
    match outcome {
        "completed" => "Completed",
        "refused" => "Refused",
        "unavailable" => "Unavailable",
        "not_started" => "Not started",
        "outcome_unknown" => "Outcome unknown",
        _ => outcome,
    }
}

fn report(export: &Export) -> String {
    html! {
        (DOCTYPE)
        html lang="en-GB" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width,initial-scale=1";
                meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'";
                title { "Common Measure comparison" }
                style { "body{font:16px/1.5 system-ui,sans-serif;color:#182e3d;background:#fff;max-width:1100px;margin:40px auto;padding:0 24px}h1{line-height:1.15}h2{margin-top:36px}table{border-collapse:collapse;width:100%}th,td{text-align:left;vertical-align:top;border-bottom:1px solid #cbd5df;padding:10px}th{background:#eef4f7}code{overflow-wrap:anywhere}p{max-width:85ch}.notice{border-left:4px solid #24677d;padding:12px 16px;background:#eef4f7}.scroll{overflow-x:auto}@media print{body{margin:0;padding:0;font-size:11pt}thead{display:table-header-group}tr{break-inside:avoid}}" }
            }
            body {
                p { "Common Measure · Results export" }
                h1 { "Retrieval comparison" }
                p class="notice" {
                    @if export.complete { "All selected provider operations have a recorded outcome. See each provider's status." }
                    @else { "Incomplete comparison. An unfinished operation may have incurred a charge. This report does not retry it." }
                }
                @if export.evidence_gap_count != 0 || export.allowance_gap_count != 0 {
                    p class="notice" { "Recorded gaps: " (export.evidence_gap_count) " evidence; " (export.allowance_gap_count) " allowance accounting. Provider outcomes do not establish complete evidence or complete allowance accounting. Details remain in the private source record." }
                }
                p { "Comparison " code { (export.comparison_id) } }
                p { "Started: " (export.started_at.as_deref().unwrap_or("not recorded")) " · Finished: " (export.finished_at.as_deref().unwrap_or("not recorded")) }
                p { "Requested results per provider: " (export.requested_limit.map(|v| v.to_string()).unwrap_or_else(|| "not recorded".into())) " · Effective limit: " (export.effective_limit.map(|v| v.to_string()).unwrap_or_else(|| "not recorded".into())) }
                p { "Policy mode: " (export.policy_mode.as_deref().unwrap_or("not recorded")) }
                @if let Some(query) = &export.query { h2 { "Query included by the operator" } p { (query) } p { "Query text is included exactly as entered. Review it for private information before sharing." } }
                @else { p { "Query text is excluded from this export." } }
                @for provider in &export.providers {
                    h2 { (provider.provider) }
                    p { "Outcome: " strong { (outcome_label(provider.outcome)) } " · Adapter: " (provider.adapter_version.as_deref().unwrap_or("not recorded")) }
                    div class="scroll" { table {
                        thead { tr { th { "Measurement" } th { "Value" } th { "Unit" } th { "Basis / missing evidence" } } }
                        tbody { @for metric in &provider.metrics { tr {
                            td { (metric_label(metric.name)) }
                            td { (metric.value.as_ref().map(ToString::to_string).unwrap_or_else(|| "Unknown".into())) }
                            td { (metric.unit) }
                            td { (if metric.missing_reason.is_some() { "Not recorded" } else { metric.basis }) }
                        } } }
                    } }
                }
                h2 { "What this establishes" }
                p { "This is a retrieval probe. Answer correctness and document relevance were not evaluated. No benchmark score is claimed." }
                p { "Costs are recorded per provider and unit. Unknown cost is not zero; quoted amounts are not observed charges. No complete bill or cross-currency total is implied." }
                p { "Source passage, result URL, answer, credential, local path and private policy fields are excluded. Optional query text may itself contain private information. It can be forwarded and cannot be revoked. Exporting makes no supplier or model calls." }
                p { "Produced by Common Measure " (export.producer_version) " · " code { (export.schema) } }
            }
        }
    }.into_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonmeasure_harness::compare::SCHEMA;
    use std::io::Read;

    fn record() -> Value {
        json!({"evidence":[
            {"event":"comparison_started","payload":{"schema":SCHEMA,
                "comparison_id":"compare-53e820c0-2840-47f4-9ff5-efc952463071",
                "query":"=HYPERLINK(\"https://example.test\")\n<script>alert(1)</script>",
                "selected_providers":["exa"],"requested_limit":5,"effective_limit":5,
                "principal":"SECRET_PRINCIPAL","cwd":"SECRET_CWD","query_sha256":"SECRET_HASH"}},
            {"event":"comparison_provider_started","payload":{"provider":"exa"}},
            {"event":"comparison_provider_finished","payload":{"provider":"exa","status":"unavailable",
                "error":"SECRET_ERROR","endpoint":"SECRET_ENDPOINT","charge":null}},
            {"event":"comparison_finished","payload":{}}
        ]})
    }

    fn files(bytes: Vec<u8>) -> std::collections::BTreeMap<String, Vec<u8>> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        (0..zip.len())
            .map(|i| {
                let mut entry = zip.by_index(i).unwrap();
                let name = entry.name().to_owned();
                let mut data = Vec::new();
                entry.read_to_end(&mut data).unwrap();
                (name, data)
            })
            .collect()
    }

    #[test]
    fn archive_is_allow_listed_and_its_manifest_matches_every_file() {
        let mut retained = record();
        retained["evidence"]
            .as_array_mut()
            .unwrap()
            .push(json!({"event":"allowance_gap","payload":{"detail":"SECRET_LEDGER"}}));
        let files = files(archive(&retained, false).unwrap());
        assert_eq!(
            files.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["cases.csv", "manifest.json", "report.html", "summary.csv"]
        );
        for bytes in files.values() {
            let text = std::str::from_utf8(bytes).unwrap();
            assert!(!text.contains("SECRET_"));
            assert!(!text.contains("HYPERLINK"));
        }
        let manifest: Value = serde_json::from_slice(&files["manifest.json"]).unwrap();
        for file in manifest["files"].as_array().unwrap() {
            assert_eq!(
                file["sha256"],
                sha256_digest(&files[file["name"].as_str().unwrap()])
            );
        }
        assert_eq!(
            manifest["result"]["providers"][0]["metrics"][5]["value"],
            Value::Null
        );
        let report = std::str::from_utf8(&files["report.html"]).unwrap();
        assert!(report.contains("Unknown"));
        assert!(report.contains("Recorded gaps:"));
        assert_eq!(manifest["result"]["allowance_gap_count"], 1);
        assert!(
            std::str::from_utf8(&files["summary.csv"])
                .unwrap()
                .contains("allowance_gap_count")
        );
        assert!(report.contains("were not evaluated"));
        assert!(
            std::str::from_utf8(&files["summary.csv"])
                .unwrap()
                .contains("price_not_recorded")
        );
    }

    #[test]
    fn query_opt_in_escapes_html_and_spreadsheet_formulas() {
        let files = files(archive(&record(), true).unwrap());
        let html = std::str::from_utf8(&files["report.html"]).unwrap();
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
        let csv = std::str::from_utf8(&files["cases.csv"]).unwrap();
        assert!(csv.contains("\"'=HYPERLINK(\"\"https://example.test\"\")"));
        let manifest: Value = serde_json::from_slice(&files["manifest.json"]).unwrap();
        assert_eq!(
            manifest["result"]["query"],
            record()["evidence"][0]["payload"]["query"]
        );
        assert!(
            !serde_json::to_string(&manifest)
                .unwrap()
                .contains("SECRET_")
        );
    }
}
