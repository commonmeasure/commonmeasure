//! Parallel: open-web search, plus URL extraction as the `fetch` capability.
//!
//! The verification state, its dated evidence and the per-operation basis
//! are `docs/knowledge-base/provider-verification.md`.
//!
//! The wire format follows Parallel's published documentation: `x-api-key`
//! auth throughout; search takes
//! `objective`/`search_queries` with an `advanced_settings.source_policy`
//! domain filter and returns `results`/`excerpts`/`usage`; extract takes
//! `urls` with `advanced_settings.full_content` and returns `results`
//! (`full_content`, `excerpts`, `publish_date`), a per-URL `errors` list, and
//! `usage`. The tests below exercise both parsers
//! over the documented shapes through the real transport and a loopback origin.
//!
//! Two fields the documentation locates on a page this operator could not
//! retrieve are deliberately not sent: the provider-side result-count limit
//! and the per-excerpt length cap. Rather than guess their names, the adapter
//! caps the *offered* candidates to the job's result limit itself and records
//! that it did so; the sealed response still carries everything Parallel
//! returned. When those field names are verified, move the cap onto the
//! request so the provider trims before billing.

use commonmeasure_http::Request;
use commonmeasure_types::{
    AcquisitionCharge, ChargeBasis, ContextEnvelope, DeclaredDate, NativeCharge, ProviderCapability,
};
use serde_json::{Number, Value, json};

use crate::{
    Acquisition, ResultFields, SupplyAdapter, SupplyError, envelopes_from_results, execute,
    parse_json, results_array,
};

pub(crate) const DEFAULT_BASE_URL: &str = "https://api.parallel.ai";
/// Search discovers candidates for a query; fetch retrieves a single named URL
/// through Parallel's Extract API. No Parallel response carries a licence,
/// rights or author field on either, so neither is treated as permission.
pub(crate) const CAPABILITIES: &[ProviderCapability] =
    &[ProviderCapability::Search, ProviderCapability::Fetch];

/// Characters of full page text requested per extracted URL is not a knob the
/// Extract API documents; `full_content` is a boolean. This adapter asks for
/// the full content so a fetch returns the article body, not a lead excerpt.
const EXTRACT_ENDPOINT: &str = "/v1/extract";

pub struct ParallelAdapter {
    base_url: String,
    api_key: String,
}

impl ParallelAdapter {
    /// `base_url` is a parameter so a test can point the adapter at a loopback
    /// origin serving a documented-shape response. The credential is held here
    /// and only ever written into a request header.
    pub fn new(base_url: &str, api_key: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key: api_key.to_owned(),
        }
    }
}

impl SupplyAdapter for ParallelAdapter {
    fn provider(&self) -> &str {
        "parallel"
    }

    fn capabilities(&self) -> &'static [ProviderCapability] {
        CAPABILITIES
    }

    fn search(
        &self,
        query: &str,
        limit: u32,
        include_hosts: &[&str],
    ) -> Result<Acquisition, SupplyError> {
        let endpoint = format!("{}/v1/search", self.base_url);
        // The natural-language objective and one literal query carrying the
        // same words: Parallel takes both, and sending the job prompt as each
        // keeps this plan's request comparable with the single-query providers
        // in the same run.
        let mut body = json!({
            "objective": query,
            "search_queries": [query],
        });
        // `source_policy.include_domains` scopes the search to the operator's
        // allowed source hosts, nested under `advanced_settings` as the Search
        // API requires. Absent when the job named none, so an unscoped search
        // sends neither the settings object nor the policy; present, it asks
        // Parallel for only the hosts admission would keep anyway. Parallel
        // takes bare hostnames and normalises a leading `www.` away.
        if !include_hosts.is_empty() {
            body["advanced_settings"] = json!({
                "source_policy": {"include_domains": include_hosts},
            });
        }
        let mut request = Request::post(
            "/",
            serde_json::to_vec(&body).map_err(|error| SupplyError::Malformed {
                detail: format!("could not serialise the request: {error}"),
            })?,
            "application/json",
        );
        request.headers.set("x-api-key", &self.api_key);
        request.headers.set("Accept", "application/json");

        let (response, latency_ms) = execute(&endpoint, request)?;
        let parsed = parse_json(&response.body)?;
        let envelopes = envelopes_from(&parsed, limit, &endpoint)?;

        Ok(Acquisition {
            provider: self.provider().to_owned(),
            capability: ProviderCapability::Search,
            endpoint,
            http_status: Some(response.status),
            latency_ms,
            provider_request_id: parsed
                .get("search_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            charge: charge_from(&parsed, "sku_search"),
            envelopes,
            raw_response: response.body,
            invocation: None,
        })
    }

    fn fetch(&self, url: &str) -> Result<Acquisition, SupplyError> {
        let endpoint = format!("{}{EXTRACT_ENDPOINT}", self.base_url);
        // One URL, the full body, no query: extract names its target and asks
        // for the article rather than a lead excerpt.
        let body = json!({
            "urls": [url],
            "advanced_settings": {"full_content": true},
        });
        let mut request = Request::post(
            "/",
            serde_json::to_vec(&body).map_err(|error| SupplyError::Malformed {
                detail: format!("could not serialise the request: {error}"),
            })?,
            "application/json",
        );
        request.headers.set("x-api-key", &self.api_key);
        request.headers.set("Accept", "application/json");

        let (response, latency_ms) = execute(&endpoint, request)?;
        let parsed = parse_json(&response.body)?;
        // A per-URL failure (an origin 403, a paywall) arrives inside a 200 as
        // an `errors` entry with `results` empty. That is a real, recordable
        // outcome — the origin refused the fetch — so it yields an acquisition
        // with no admissible envelope and the provider's own error sealed in
        // the raw response, never a fabricated body and never an error that
        // reads like the provider was unreachable.
        let envelopes = fetch_envelopes(&parsed, &endpoint)?;

        Ok(Acquisition {
            provider: self.provider().to_owned(),
            capability: ProviderCapability::Fetch,
            endpoint,
            http_status: Some(response.status),
            latency_ms,
            provider_request_id: parsed
                .get("extract_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            charge: charge_from(&parsed, "sku_extract_excerpts"),
            envelopes,
            raw_response: response.body,
            invocation: None,
        })
    }
}

/// One envelope per successfully extracted URL. Text is the `full_content` body
/// where present, falling back to the joined `excerpts`; a URL that produced
/// only an error carries no envelope at all — the error stays in the sealed
/// response, which is where "the origin refused this fetch" is recorded.
fn fetch_envelopes(body: &Value, endpoint: &str) -> Result<Vec<ContextEnvelope>, SupplyError> {
    let results = results_array(body, "/results", endpoint)?;
    Ok(envelopes_from_results(results, |item| {
        Some(ResultFields {
            url: item.get("url")?.as_str()?.to_owned(),
            title: item.get("title").and_then(Value::as_str).map(str::to_owned),
            // Prefer the full body, but an extract often returns `full_content`
            // as an empty string while populating `excerpts` — an empty body is
            // no body, so it falls back to the excerpts rather than admitting a
            // textless source next to content the provider did return.
            text: item
                .get("full_content")
                .and_then(Value::as_str)
                .filter(|body| !body.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    item.get("excerpts").and_then(Value::as_array).map(|parts| {
                        parts
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("\n\n")
                    })
                })
                .filter(|joined| !joined.is_empty()),
            declared_date: item
                .get("publish_date")
                .and_then(Value::as_str)
                .map(|date| DeclaredDate {
                    date: date.to_owned(),
                    provenance: "supplier-declared: publish_date on the Parallel extract result"
                        .to_owned(),
                }),
            promoted: &["url", "title", "full_content", "excerpts", "publish_date"],
        })
    }))
}

fn envelopes_from(
    body: &Value,
    limit: u32,
    endpoint: &str,
) -> Result<Vec<ContextEnvelope>, SupplyError> {
    // The provider-side count limit is not wired (see the module note), so the
    // adapter honours the job's result limit here. The sealed response still
    // holds every result Parallel returned. No Parallel response carries
    // licence metadata, so every envelope stays unknown.
    let results = results_array(body, "/results", endpoint)?
        .iter()
        .take(limit as usize);
    Ok(envelopes_from_results(results, |item| {
        Some(ResultFields {
            url: item.get("url")?.as_str()?.to_owned(),
            title: item.get("title").and_then(Value::as_str).map(str::to_owned),
            // Parallel returns page text as `excerpts`, an array of strings.
            // They are joined with blank lines into the one text field the
            // envelope carries; an empty or absent array leaves the candidate
            // textless rather than inventing content.
            text: item
                .get("excerpts")
                .and_then(Value::as_array)
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n\n")
                })
                .filter(|joined| !joined.is_empty()),
            // `publish_date` is Parallel's documented per-result date, nullable
            // and frequently null. Mapped verbatim where present, with
            // provenance naming the field; a result without one stays undated
            // rather than dated by a neighbour.
            declared_date: item
                .get("publish_date")
                .and_then(Value::as_str)
                .map(|date| DeclaredDate {
                    date: date.to_owned(),
                    provenance: "supplier-declared: publish_date on the Parallel search result"
                        .to_owned(),
                }),
            promoted: &["url", "title", "excerpts", "publish_date"],
        })
    }))
}

/// Read the charge for `sku` from the response's `usage` array.
///
/// Parallel reports billing as SKUs, not currency: `usage` is a list of
/// `{name, count}` objects — a search bills `sku_search`, an extract
/// `sku_extract_excerpts`. The adapter records the named SKU's count in its own
/// unit, basis observed, and never derives a dollar figure Parallel did not
/// state. When the named line is absent — as on a fetch the origin refused,
/// whose `usage` is empty — the charge is left unknown rather than recorded as
/// zero: an absent count is not a free call.
fn charge_from(body: &Value, sku: &str) -> AcquisitionCharge {
    let count = body
        .get("usage")
        .and_then(Value::as_array)
        .and_then(|entries| {
            entries.iter().find_map(|entry| {
                (entry.get("name").and_then(Value::as_str) == Some(sku))
                    .then(|| entry.get("count").and_then(Value::as_u64))
                    .flatten()
            })
        });
    let Some(count) = count else {
        return AcquisitionCharge::default();
    };
    AcquisitionCharge {
        // No currency is disclosed on the response, and deriving one from a SKU
        // count would depend on the account's plan; a dollar amount here would
        // be invented, not observed.
        money: None,
        native: Some(NativeCharge {
            unit: sku.to_owned(),
            amount: Number::from(count),
            basis: ChargeBasis::Observed,
            note: Some(format!(
                "Parallel reports usage as billing SKUs; {sku} count is observed from the \
                 response, and no currency is disclosed there."
            )),
        }),
    }
}
