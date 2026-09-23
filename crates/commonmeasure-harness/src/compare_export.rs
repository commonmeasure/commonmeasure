//! Purpose-limited export of a retained retrieval comparison.
//!
//! Build a new typed document rather than removing private fields from a local
//! record. New evidence fields therefore stay private until explicitly added.

use chrono::{DateTime, Utc};
use commonmeasure_types::AcquisitionCharge;
use serde::Serialize;
use serde_json::{Number, Value};

/// Version of the portable export, independent of the private evidence format.
pub const SCHEMA: &str = "contextops-comparison-export/v1";

/// One measurement, preserving absence and the basis of a known value.
#[derive(Debug, Serialize)]
pub struct Metric {
    pub name: &'static str,
    pub value: Option<Number>,
    pub unit: String,
    pub basis: &'static str,
    pub missing_reason: Option<&'static str>,
}

/// A selected provider, including an operation that never finished or started.
#[derive(Debug, Serialize)]
pub struct ProviderResult {
    pub provider: String,
    pub outcome: &'static str,
    pub attempt: Option<u32>,
    pub adapter_version: Option<String>,
    pub metrics: Vec<Metric>,
}

/// The only data available to report renderers and archive writers.
#[derive(Debug, Serialize)]
pub struct Export {
    pub schema: &'static str,
    pub producer_version: &'static str,
    pub comparison_id: String,
    pub mode: &'static str,
    pub complete: bool,
    pub evidence_gap_count: usize,
    pub allowance_gap_count: usize,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub requested_limit: Option<u64>,
    pub effective_limit: Option<u64>,
    pub policy_mode: Option<String>,
    pub disclosure: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    pub providers: Vec<ProviderResult>,
}

/// Project an existing `compare::read` result, without performing acquisition.
///
/// Internal comparisons cannot be exported in this first disclosure profile.
/// Unknown formats or inconsistent operation sequences fail instead of producing
/// a report that appears complete. Free-text errors and charge notes never leave.
pub fn project(record: &Value, include_query: bool) -> Result<Export, String> {
    let invalid = || "Comparison evidence cannot be exported in this format.".to_owned();
    let records = record["evidence"].as_array().ok_or_else(invalid)?;
    let starts: Vec<_> = records
        .iter()
        .filter(|r| r["event"] == "comparison_started")
        .collect();
    if starts.len() != 1 {
        return Err(invalid());
    }
    let start = &starts[0]["payload"];
    if start["schema"] != crate::compare::SCHEMA {
        return Err(invalid());
    }
    let id = start["comparison_id"].as_str().ok_or_else(invalid)?;
    let uuid = id
        .strip_prefix("compare-")
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .ok_or_else(invalid)?;
    let selected = start["selected_providers"].as_array().ok_or_else(invalid)?;
    if selected.is_empty() {
        return Err(invalid());
    }
    if records.iter().any(|r| {
        (matches!(
            r["event"].as_str(),
            Some("crossing_mediated" | "crossing_refused")
        ) && r["payload"]["internal"] == true)
            || (r["event"] == "comparison_provider_finished"
                && (r["payload"]["metadata_withheld"]
                    .as_u64()
                    .is_some_and(|n| n > 0)
                    || r["payload"]["refused"]
                        .as_u64()
                        .zip(r["payload"]["refusals"].as_array())
                        .is_some_and(|(count, recorded)| count > recorded.len() as u64)))
    }) {
        return Err("Internal-source comparisons cannot be exported.".to_owned());
    }
    let mut providers = Vec::new();
    for selected in selected {
        let provider = selected.as_str().ok_or_else(invalid)?;
        if provider == "internal" {
            return Err("Internal-source comparisons cannot be exported.".to_owned());
        }
        if !commonmeasure_supply::IMPLEMENTED_PROVIDERS.contains(&provider)
            || providers
                .iter()
                .any(|r: &ProviderResult| r.provider == provider)
        {
            return Err(invalid());
        }
        let events: Vec<_> = records
            .iter()
            .filter(|r| {
                r["payload"]["provider"] == provider
                    && matches!(
                        r["event"].as_str(),
                        Some("comparison_provider_started" | "comparison_provider_finished")
                    )
            })
            .collect();
        let (attempt, result) = match events.as_slice() {
            [] => (None, None),
            [started] if started["event"] == "comparison_provider_started" => (Some(1), None),
            [started, finished]
                if started["event"] == "comparison_provider_started"
                    && finished["event"] == "comparison_provider_finished" =>
            {
                (Some(1), Some(&finished["payload"]))
            }
            _ => return Err(invalid()),
        };
        let outcome = match result.and_then(|r| r["status"].as_str()) {
            Some("completed") => "completed",
            Some("refused") => "refused",
            Some("unavailable") => "unavailable",
            None if attempt.is_none() => "not_started",
            None if result.is_none() => "outcome_unknown",
            _ => return Err(invalid()),
        };
        let empty = Value::Null;
        let result = result.unwrap_or(&empty);
        let mut metrics = Vec::new();
        for (name, key, unit, basis) in [
            ("received", "received", "results", "recorded acquisition"),
            ("admitted", "results_count", "results", "recorded admission"),
            ("refused", "refused", "results", "recorded admission"),
            (
                "request_latency",
                "latency_ms",
                "ms",
                "adapter request wall-clock",
            ),
            (
                "elapsed",
                "elapsed_ms",
                "ms",
                "policy, acquisition and recording wall-clock",
            ),
        ] {
            let value = result[key].as_u64().map(Number::from);
            metrics.push(Metric {
                name,
                missing_reason: value.is_none().then_some("not_recorded"),
                value,
                unit: unit.into(),
                basis,
            });
        }
        let charge: AcquisitionCharge =
            serde_json::from_value(result["charge"].clone()).unwrap_or_default();
        if let Some(money) = &charge.money {
            metrics.push(Metric {
                name: "acquisition_cost",
                value: Some(money.as_decimal_string().parse().map_err(|_| invalid())?),
                unit: safe_token(money.currency()).ok_or_else(invalid)?,
                basis: "observed",
                missing_reason: None,
            });
        }
        if let Some(native) = &charge.native {
            metrics.push(Metric {
                name: "acquisition_cost_native",
                value: Some(native.amount.clone()),
                unit: safe_token(&native.unit).ok_or_else(invalid)?,
                basis: match native.basis {
                    commonmeasure_types::ChargeBasis::Observed => "observed",
                    commonmeasure_types::ChargeBasis::Quoted => "quoted",
                },
                missing_reason: None,
            });
        }
        if charge.money.is_none() && charge.native.is_none() {
            metrics.push(Metric {
                name: "acquisition_cost",
                value: None,
                unit: "unknown".into(),
                basis: "unknown",
                missing_reason: Some("price_not_recorded"),
            });
        }
        providers.push(ProviderResult {
            provider: provider.into(),
            outcome,
            attempt,
            adapter_version: result["adapter_version"].as_str().and_then(adapter_version),
            metrics,
        });
    }
    let finishes: Vec<_> = records
        .iter()
        .filter(|r| r["event"] == "comparison_finished")
        .collect();
    if finishes.len() > 1 {
        return Err(invalid());
    }
    let complete = !finishes.is_empty();
    if complete
        && providers
            .iter()
            .any(|p| matches!(p.outcome, "not_started" | "outcome_unknown"))
    {
        return Err(invalid());
    }
    let query = if include_query {
        Some(
            start["query"]
                .as_str()
                .filter(|q| q.len() <= 4096)
                .ok_or_else(invalid)?
                .to_owned(),
        )
    } else {
        None
    };
    let mode = start["policy"]["policy_mode"]
        .as_str()
        .or_else(|| record["policy_mode"].as_str());
    Ok(Export {
        schema: SCHEMA,
        producer_version: env!("CARGO_PKG_VERSION"),
        comparison_id: format!("compare-{uuid}"),
        mode: "retrieval_probe",
        complete,
        evidence_gap_count: records
            .iter()
            .filter(|r| r["event"] == "evidence_gap")
            .count(),
        allowance_gap_count: records
            .iter()
            .filter(|r| r["event"] == "allowance_gap")
            .count(),
        started_at: timestamp(&starts[0]["timestamp"]),
        finished_at: finishes.first().and_then(|r| timestamp(&r["timestamp"])),
        requested_limit: start["requested_limit"].as_u64(),
        effective_limit: start["effective_limit"].as_u64(),
        policy_mode: mode
            .filter(|m| matches!(*m, "strict" | "observe" | "prefer"))
            .map(str::to_owned),
        disclosure: if include_query {
            "metrics_and_query"
        } else {
            "metrics_only"
        },
        query,
        providers,
    })
}

fn safe_token(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b)))
    .then(|| value.to_owned())
}

fn adapter_version(value: &str) -> Option<String> {
    if let Some((adapter, version)) = value.split_once('/') {
        safe_token(adapter)?;
        safe_token(version)?;
        Some(value.to_owned())
    } else {
        safe_token(value)
    }
}

fn timestamp(value: &Value) -> Option<String> {
    DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|t| t.with_timezone(&Utc).to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn retained() -> Value {
        json!({"policy_mode":"strict", "evidence":[
            {"event":"comparison_started", "timestamp":"2026-09-22T12:00:00Z", "payload":{
                "schema":crate::compare::SCHEMA,
                "comparison_id":"compare-53e820c0-2840-47f4-9ff5-efc952463071",
                "query":"PRIVATE_QUERY", "query_sha256":"PRIVATE_HASH", "home":"/PRIVATE_HOME",
                "principal":"PRIVATE_PERSON", "policy":{"secret":"PRIVATE_POLICY"},
                "selected_providers":["exa","tavily","you"], "requested_limit":5,"effective_limit":5}},
            {"event":"comparison_provider_started", "payload":{"provider":"exa"}},
            {"event":"comparison_provider_finished", "payload":{"provider":"exa","status":"completed",
                "results_count":0,"received":2,"refused":2,"latency_ms":42,"elapsed_ms":50,
                "charge":{"money":{"currency":"USD","micros":0},"native":{"unit":"credits","amount":2,"basis":"quoted","note":"PRIVATE_NOTE"}},
                "adapter_version":commonmeasure_supply::ADAPTER_VERSION,"endpoint":"https://PRIVATE_ENDPOINT","refusals":["PRIVATE_REASON", "PRIVATE_REASON_2"],
                "results":[{"url":"https://PRIVATE_URL","text":"PRIVATE_PASSAGE"}],"extra":"PRIVATE_NEW_FIELD"}},
            {"event":"comparison_provider_started", "payload":{"provider":"tavily"}}
        ]})
    }

    #[test]
    fn projection_withholds_private_fields_and_preserves_zero_and_interrupted_work() {
        let mut record = retained();
        record["evidence"]
            .as_array_mut()
            .unwrap()
            .push(json!({"event":"allowance_gap","payload":{"detail":"PRIVATE_LEDGER"}}));
        record["evidence"]
            .as_array_mut()
            .unwrap()
            .push(json!({"event":"evidence_gap","payload":{"detail":"PRIVATE_WRITE"}}));
        let export = project(&record, false).unwrap();
        assert_eq!(export.allowance_gap_count, 1);
        assert_eq!(export.evidence_gap_count, 1);
        let serialised = serde_json::to_string(&export).unwrap();
        assert!(!serialised.contains("PRIVATE_"));
        assert!(!export.complete);
        assert_eq!(
            export.providers[0].adapter_version.as_deref(),
            Some(commonmeasure_supply::ADAPTER_VERSION)
        );
        assert_eq!(export.providers[0].metrics[1].value, Some(Number::from(0)));
        assert_eq!(
            export.providers[0].metrics[5]
                .value
                .as_ref()
                .and_then(Number::as_f64),
            Some(0.0)
        );
        assert_eq!(export.providers[0].metrics[6].basis, "quoted");
        assert_eq!(export.providers[1].outcome, "outcome_unknown");
        assert_eq!(export.providers[1].attempt, Some(1));
        assert!(
            export.providers[1]
                .metrics
                .iter()
                .all(|m| m.value.is_none())
        );
        assert_eq!(export.providers[2].outcome, "not_started");
        assert_eq!(export.providers[2].attempt, None);
        assert!(
            serde_json::to_string(&project(&retained(), true).unwrap())
                .unwrap()
                .contains("PRIVATE_QUERY")
        );
    }

    #[test]
    fn remote_providers_with_internal_or_withheld_results_cannot_export() {
        for event in ["crossing_mediated", "crossing_refused"] {
            let mut record = retained();
            record["evidence"].as_array_mut().unwrap().push(json!({
                "event":event, "payload":{"provider":"exa", "internal":true}
            }));
            assert!(project(&record, false).unwrap_err().contains("Internal"));
            assert!(project(&record, true).is_err());
        }
        let mut record = retained();
        record["evidence"][2]["payload"]["metadata_withheld"] = json!(1);
        assert!(project(&record, false).unwrap_err().contains("Internal"));
        record["evidence"][2]["payload"]["metadata_withheld"] = json!(0);
        assert!(project(&record, false).is_ok());
        // Private refusals are counted but their crossing and refusal details
        // are omitted by the recording floor.
        record["evidence"][2]["payload"]["refusals"] = json!([]);
        assert!(project(&record, false).unwrap_err().contains("Internal"));
    }

    #[test]
    fn internal_unknown_and_contradictory_evidence_are_not_exported() {
        let mut record = retained();
        record["evidence"][0]["payload"]["selected_providers"] = json!(["internal"]);
        assert!(project(&record, true).unwrap_err().contains("Internal"));
        record = retained();
        record["evidence"][0]["payload"]["schema"] = json!("future-schema");
        assert!(project(&record, false).is_err());
        record = retained();
        record["evidence"]
            .as_array_mut()
            .unwrap()
            .push(json!({"event":"comparison_finished","payload":{}}));
        assert!(
            project(&record, false).is_err(),
            "a completed marker cannot hide unfinished providers"
        );
    }
}
