//! Tavily: open-web search.
//!
//! The verification state, its dated evidence and the per-operation basis
//! are `docs/knowledge-base/provider-verification.md`.
//!
//! Neither the search nor the `/extract` response carries a cost or credit
//! field, and the account usage counters did not move across a billed search,
//! so no Tavily charge is observed.

use commonmeasure_http::Request;
use commonmeasure_types::{AcquisitionCharge, ContextEnvelope, ProviderCapability};
use serde_json::{Number, Value, json};

use crate::{
    Acquisition, ResultFields, SupplyAdapter, SupplyError, envelopes_from_results, execute,
    parse_json, quoted_native, results_array,
};

pub(crate) const DEFAULT_BASE_URL: &str = "https://api.tavily.com";
/// Search discovers candidates; fetch retrieves a named URL through `/extract`.
pub(crate) const CAPABILITIES: &[ProviderCapability] =
    &[ProviderCapability::Search, ProviderCapability::Fetch];

pub struct TavilyAdapter {
    base_url: String,
    api_key: String,
}

impl TavilyAdapter {
    pub fn new(base_url: &str, api_key: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key: api_key.to_owned(),
        }
    }
}

impl SupplyAdapter for TavilyAdapter {
    fn provider(&self) -> &str {
        "tavily"
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
        let endpoint = format!("{}/search", self.base_url);
        // Basic depth: the cheapest documented tier, one credit. Advanced
        // depth costs two and would put this plan on a different price basis
        // from the rest of the run.
        let mut body = json!({"query": query, "max_results": limit, "search_depth": "basic"});
        // `include_domains` scopes the search to the operator's allowed source
        // hosts. Absent when the job named none, so an unscoped search sends the
        // body it always did; present, it asks Tavily for only the hosts
        // admission would keep anyway.
        if !include_hosts.is_empty() {
            body["include_domains"] = json!(include_hosts);
        }
        let mut request = Request::post(
            "/",
            serde_json::to_vec(&body).map_err(|error| SupplyError::Malformed {
                detail: format!("could not serialise the request: {error}"),
            })?,
            "application/json",
        );
        request
            .headers
            .set("Authorization", &format!("Bearer {}", self.api_key));
        request.headers.set("Accept", "application/json");

        let (response, latency_ms) = execute(&endpoint, request)?;
        let parsed = parse_json(&response.body)?;
        let envelopes = envelopes_from(&parsed, &endpoint)?;

        Ok(Acquisition {
            provider: self.provider().to_owned(),
            capability: ProviderCapability::Search,
            endpoint,
            http_status: Some(response.status),
            latency_ms,
            provider_request_id: parsed
                .get("request_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            charge: quoted_charge(),
            envelopes,
            raw_response: response.body,
            invocation: None,
        })
    }

    fn fetch(&self, url: &str) -> Result<Acquisition, SupplyError> {
        let endpoint = format!("{}/extract", self.base_url);
        let body = json!({"urls": [url]});
        let mut request = Request::post(
            "/",
            serde_json::to_vec(&body).map_err(|error| SupplyError::Malformed {
                detail: format!("could not serialise the request: {error}"),
            })?,
            "application/json",
        );
        request
            .headers
            .set("Authorization", &format!("Bearer {}", self.api_key));
        request.headers.set("Accept", "application/json");

        let (response, latency_ms) = execute(&endpoint, request)?;
        let parsed = parse_json(&response.body)?;
        let envelopes = extract_envelopes(&parsed, &endpoint)?;

        Ok(Acquisition {
            provider: self.provider().to_owned(),
            capability: ProviderCapability::Fetch,
            endpoint,
            http_status: Some(response.status),
            latency_ms,
            provider_request_id: parsed
                .get("request_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            charge: extract_charge(&parsed),
            envelopes,
            raw_response: response.body,
            invocation: None,
        })
    }
}

/// One envelope per successfully extracted URL. `/extract` returns the full page
/// body as `raw_content`; a URL that failed lands in `failed_results` and
/// carries no envelope. An empty `raw_content` yields a textless envelope (no
/// text, no content hash), never invented content.
fn extract_envelopes(body: &Value, endpoint: &str) -> Result<Vec<ContextEnvelope>, SupplyError> {
    let results = results_array(body, "/results", endpoint)?;
    Ok(envelopes_from_results(results, |item| {
        Some(ResultFields {
            url: item.get("url")?.as_str()?.to_owned(),
            title: item.get("title").and_then(Value::as_str).map(str::to_owned),
            // An empty string stays an empty string
            // (`commonmeasure_types::ContextEnvelope::text`).
            text: item
                .get("raw_content")
                .and_then(Value::as_str)
                .map(str::to_owned),
            declared_date: None,
            promoted: &["url", "title", "raw_content"],
        })
    }))
}

/// No licence, publisher, author or rights field on any endpoint tested, so
/// every envelope stays unknown.
fn envelopes_from(body: &Value, endpoint: &str) -> Result<Vec<ContextEnvelope>, SupplyError> {
    let results = results_array(body, "/results", endpoint)?;
    Ok(envelopes_from_results(results, |item| {
        Some(ResultFields {
            url: item.get("url")?.as_str()?.to_owned(),
            title: item.get("title").and_then(Value::as_str).map(str::to_owned),
            // An empty string stays an empty string
            // (`commonmeasure_types::ContextEnvelope::text`).
            text: item
                .get("content")
                .and_then(Value::as_str)
                .map(str::to_owned),
            // No date field: Tavily documents `published_date` only for
            // news-topic searches, which this adapter does not request, and
            // the recorded general search carries none (`demo/recon/tavily/`).
            // Nothing is mapped that has never been seen.
            declared_date: None,
            // `score` stays in native metadata rather than being promoted: a
            // provider's own relevance number is not comparable across
            // providers.
            promoted: &["url", "title", "content"],
        })
    }))
}

/// The extract response carries no cost field, and `/extract` bills on a
/// different basis from search.
///
/// Tavily's API-credits page (<https://docs.tavily.com/documentation/api-credits>,
/// read 4 September 2026) states the basic-extract rate as "every 5 successful
/// URL extractions cost 1 API credit" and that a failed extraction is never
/// charged. It states no rule for a call with fewer than five successful
/// extractions, so the credit count of this single-URL call is not documented:
/// one whole credit and a fifth of one would each assert a rounding rule the
/// page does not make. A call whose URL was extracted therefore records an
/// unknown charge (`docs/knowledge-base/provider-verification.md` records why),
/// which the ledger treats as unpriced, never as free. A call whose only URL
/// failed is the documented zero and is quoted as such.
fn extract_charge(body: &Value) -> AcquisitionCharge {
    let extracted = body
        .get("results")
        .and_then(Value::as_array)
        .is_some_and(|results| !results.is_empty());
    let failed = body
        .get("failed_results")
        .and_then(Value::as_array)
        .is_some_and(|failed| !failed.is_empty());
    if !extracted && failed {
        return quoted_native(
            "credits",
            Number::from(0u64),
            "Tavily reports no cost on the extract response; the URL landed in failed_results, \
             and the published API-credits page states that a failed extraction is never \
             charged (read 4 September 2026).",
        );
    }
    AcquisitionCharge::default()
}

/// Tavily reports no charge anywhere, so the only honest figure is the
/// published price, marked quoted. Recording it as observed would be a
/// fabrication, and recording nothing would lose the fact that this search was
/// billable.
fn quoted_charge() -> AcquisitionCharge {
    quoted_native(
        "credits",
        Number::from(1u64),
        "Tavily reports no cost on the search response and its usage counters did not move \
         across a billed search; quoted from the published basic-search price.",
    )
}
