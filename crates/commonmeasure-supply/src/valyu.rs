//! Valyu Search and Contents, built to the published OpenAPI 2.3.0.
//!
//! Search uses the documented all-source class; Contents extracts one named URL without
//! synthesis. Publisher identity and per-result USD prices stay in native
//! metadata. Neither is a machine-readable licence, so rights stay unknown.

use commonmeasure_http::Request;
use commonmeasure_types::{
    AcquisitionCharge, ChargeBasis, DeclaredDate, Money, NativeCharge, ProviderCapability,
};
use serde_json::{Value, json};

use crate::{
    Acquisition, ResultFields, SupplyAdapter, SupplyError, envelopes_from_results, error_detail,
    execute, parse_json, required_field, results_array,
};

pub(crate) const DEFAULT_BASE_URL: &str = "https://api.valyu.ai";
pub(crate) const CAPABILITIES: &[ProviderCapability] =
    &[ProviderCapability::Search, ProviderCapability::Fetch];
const MAX_RESULTS: u32 = 20;

/// Search and raw content extraction through Valyu. No Answer or DeepResearch
/// operation is dispatched by this adapter.
pub struct ValyuAdapter {
    base_url: String,
    api_key: String,
}

impl ValyuAdapter {
    /// The origin is configurable for recorded replay through the production
    /// transport. The credential travels only in `x-api-key`.
    pub fn new(base_url: &str, api_key: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key: api_key.to_owned(),
        }
    }

    fn acquire(
        &self,
        path: &str,
        body: &Value,
        capability: ProviderCapability,
        cost_field: &str,
    ) -> Result<Acquisition, SupplyError> {
        let endpoint = format!("{}{path}", self.base_url);
        let mut request = Request::post(
            "/",
            serde_json::to_vec(body).map_err(|error| SupplyError::Malformed {
                detail: format!("could not serialise the request: {error}"),
            })?,
            "application/json",
        );
        request.headers.set("x-api-key", &self.api_key);
        request.headers.set("Accept", "application/json");
        // Non-2xx responses, including Contents' all-failed 422, are explicit
        // errors. No acquisition or assumed free charge is constructed.
        let (response, latency_ms) = execute(&endpoint, request)?;
        let parsed = parse_json(&response.body)?;
        let success = required_field(&parsed, "/success", &endpoint)?
            .as_bool()
            .ok_or_else(|| SupplyError::Malformed {
                detail: format!("the response from {endpoint} has no boolean `success`"),
            })?;
        if !success {
            return Err(SupplyError::Status {
                status: response.status,
                endpoint,
                detail: error_detail(&response.body),
            });
        }
        let results = results_array(&parsed, "/results", &endpoint)?;
        if capability == ProviderCapability::Fetch {
            for item in results {
                match item.get("status").and_then(Value::as_str) {
                    Some("success" | "failed") => {}
                    _ => {
                        return Err(SupplyError::Malformed {
                            detail: format!(
                                "the response from {endpoint} carries an unreadable Contents `status`"
                            ),
                        });
                    }
                }
            }
        }
        let envelopes = envelopes_from_results(results, |item| {
            if capability == ProviderCapability::Fetch
                && item.get("status").and_then(Value::as_str) != Some("success")
            {
                return None;
            }
            Some(ResultFields {
                url: item.get("url")?.as_str()?.to_owned(),
                title: item
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|title| !title.is_empty())
                    .map(str::to_owned),
                text: item
                    .get("content")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                    .map(str::to_owned),
                declared_date: item
                    .get("publication_date")
                    .and_then(Value::as_str)
                    .filter(|date| !date.is_empty())
                    .map(|date| DeclaredDate {
                        date: date.to_owned(),
                        provenance: "supplier-declared: publication_date on the Valyu result"
                            .to_owned(),
                    }),
                // In particular, `price`, `source`, `crawl_date`, `status`
                // and extraction metadata retain their supplier field names.
                promoted: &["url", "title", "content", "publication_date"],
                ..Default::default()
            })
        });
        Ok(Acquisition {
            provider: self.provider().to_owned(),
            capability,
            endpoint,
            http_status: Some(response.status),
            latency_ms,
            provider_request_id: parsed
                .get("tx_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            charge: charge_from(&parsed, cost_field),
            envelopes,
            raw_response: response.body,
            invocation: None,
        })
    }
}

impl SupplyAdapter for ValyuAdapter {
    fn provider(&self) -> &str {
        "valyu"
    }

    fn capabilities(&self) -> &'static [ProviderCapability] {
        CAPABILITIES
    }

    fn maximum_search_results(&self) -> Option<u32> {
        Some(MAX_RESULTS)
    }

    fn search(
        &self,
        query: &str,
        limit: u32,
        include_hosts: &[&str],
    ) -> Result<Acquisition, SupplyError> {
        let mut body = json!({
            "query": query,
            "search_type": "all",
            "max_num_results": limit.clamp(1, MAX_RESULTS),
            "is_tool_call": true,
        });
        // Provider-side scoping is a hint; admission still rules on each URL.
        if !include_hosts.is_empty() {
            body["included_sources"] = json!(include_hosts);
        }
        self.acquire(
            "/v1/search",
            &body,
            ProviderCapability::Search,
            "total_deduction_dollars",
        )
    }

    fn fetch(&self, url: &str) -> Result<Acquisition, SupplyError> {
        self.acquire(
            "/v1/contents",
            // `summary` defaults to false: no AI processing is requested.
            &json!({"urls": [url], "response_length": "medium"}),
            ProviderCapability::Fetch,
            "total_cost_dollars",
        )
    }
}

/// Only the operation's own total measures the call. Per-result `price`
/// remains observed native metadata, never summed into a missing total.
fn charge_from(body: &Value, field: &str) -> AcquisitionCharge {
    let Some(total) = body.get(field).and_then(Value::as_number) else {
        return AcquisitionCharge::default();
    };
    let decimal = total.to_string();
    let money = Money::from_decimal_str("USD", &decimal);
    let note = money.is_none().then(|| {
        format!(
            "Valyu reported {decimal} USD in {field}, which the ledger cannot hold at \
             micro-dollar precision, so no comparable amount was derived from it."
        )
    });
    AcquisitionCharge {
        money,
        native: Some(NativeCharge {
            unit: "USD".to_owned(),
            amount: total.clone(),
            basis: ChargeBasis::Observed,
            note,
        }),
    }
}
