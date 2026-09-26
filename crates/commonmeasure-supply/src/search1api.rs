//! Search1API: multi-engine open-web search returning result excerpts.
//!
//! The wire format follows Search1API's published Search
//! endpoint documentation (POST `/search`, `Authorization: Bearer` auth, the
//! `query`/`search_service`/`max_results` request, the `include_sites` domain
//! filter, and the `results`/`snippet` response), read at
//! <https://docs.search1api.com/api-reference/endpoint/search> on
//! 18 August 2026. The tests beside this exercise the parser over the
//! documented shapes through the real transport and a loopback origin.
//!
//! This adapter performs a plain search and does not request crawling.
//! Search1API's `crawl_results` field turns the call into a "DeepSearch" that
//! also fetches each page's full body into the per-result `content` field, and
//! its published price is then one credit for the search plus one for every
//! page successfully crawled — a variable, per-page basis that no other
//! provider in this run shares. A plain search is a flat one published credit,
//! comparable with the other single-call providers, so this adapter leaves
//! `crawl_results` at its default and takes the always-present `snippet` as the
//! result text. The parser still prefers a populated `content` over the
//! snippet, so if a later run turns DeepSearch on the full body flows into the
//! envelope without a code change; until then `content` arrives empty and the
//! snippet is what is mapped.
//!
//! Two things the documented response does not carry are deliberately not
//! invented: there is no request or search identifier field, so
//! `provider_request_id` stays `None`; and no per-result date field, so no
//! `declared_date` is ever mapped.

use commonmeasure_http::Request;
use commonmeasure_types::canonical::sha256_digest;
use commonmeasure_types::{AcquisitionCharge, ContextEnvelope, LicenceState, ProviderCapability};
use serde_json::{Number, Value, json};

use crate::{
    Acquisition, ResultFields, SupplyAdapter, SupplyError, envelopes_from_results, execute,
    host_of, native_metadata, parse_json, quoted_native, required_field, results_array,
};

pub(crate) const DEFAULT_BASE_URL: &str = "https://api.search1api.com";
/// Search discovers candidates; fetch retrieves a named URL through `/crawl`.
/// No Search1API response carries a licence, rights or author field.
pub(crate) const CAPABILITIES: &[ProviderCapability] =
    &[ProviderCapability::Search, ProviderCapability::Fetch];

/// Search1API's documented `max_results` range is 1 to 50. The job's limit is
/// clamped into it before it reaches the wire, so a request never asks for a
/// count the provider would reject.
const MAX_RESULTS: u32 = 50;

/// The open-web engine to search. Google is the provider's own default and the
/// broadest general index; naming it keeps this plan's request explicit and
/// comparable with the other providers rather than leaving the engine implied.
const SEARCH_SERVICE: &str = "google";

pub struct Search1ApiAdapter {
    base_url: String,
    api_key: String,
}

impl Search1ApiAdapter {
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

impl SupplyAdapter for Search1ApiAdapter {
    fn provider(&self) -> &str {
        "search1api"
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
        let mut body = json!({
            "query": query,
            "search_service": SEARCH_SERVICE,
            "max_results": limit.clamp(1, MAX_RESULTS),
        });
        // `include_sites` scopes the search to the operator's allowed source
        // hosts. Absent when the job named none, so an unscoped search sends the
        // body it always did; present, it asks Search1API for only the hosts
        // admission would keep anyway.
        if !include_hosts.is_empty() {
            body["include_sites"] = json!(include_hosts);
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
            // The documented response carries no request or search identifier,
            // so there is nothing to join a provider-side receipt to.
            provider_request_id: None,
            charge: quoted_charge(),
            envelopes,
            raw_response: response.body,
            invocation: None,
        })
    }

    fn fetch(&self, url: &str) -> Result<Acquisition, SupplyError> {
        let endpoint = format!("{}/crawl", self.base_url);
        let body = json!({"url": url});
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
        let envelopes = crawl_envelope(&parsed, url, &endpoint)?;

        Ok(Acquisition {
            provider: self.provider().to_owned(),
            capability: ProviderCapability::Fetch,
            endpoint,
            http_status: Some(response.status),
            latency_ms,
            provider_request_id: None,
            charge: quoted_crawl_charge(),
            envelopes,
            raw_response: response.body,
            invocation: None,
        })
    }
}

/// The one envelope a `/crawl` produces: `results` is a single object with the
/// full page `content`. Keyed to the URL Search1API reports it crawled
/// (`results.metadata.sourceUrl`, falling back to `results.link` then the
/// requested URL). Empty content yields no envelope rather than a textless
/// source.
fn crawl_envelope(
    body: &Value,
    requested: &str,
    endpoint: &str,
) -> Result<Vec<ContextEnvelope>, SupplyError> {
    let item = required_field(body, "/results", endpoint)?;
    let url = item
        .pointer("/metadata/sourceUrl")
        .and_then(Value::as_str)
        .or_else(|| item.get("link").and_then(Value::as_str))
        .unwrap_or(requested)
        .to_owned();
    let Some(host) = host_of(&url) else {
        return Ok(Vec::new());
    };
    let text = item
        .get("content")
        .and_then(Value::as_str)
        .filter(|content| !content.is_empty())
        .map(str::to_owned);
    let Some(text) = text else {
        return Ok(Vec::new());
    };
    Ok(vec![ContextEnvelope {
        host,
        content_hash: Some(sha256_digest(text.as_bytes())),
        title: item.get("title").and_then(Value::as_str).map(str::to_owned),
        text: Some(text),
        licence: LicenceState::Unknown,
        declared_date: None,
        native_metadata: native_metadata(item, &["content"]),
        retrieval_rank: 1,
        source_url: url,
    }])
}

/// No Search1API response carries licence metadata, so every envelope stays
/// unknown.
fn envelopes_from(body: &Value, endpoint: &str) -> Result<Vec<ContextEnvelope>, SupplyError> {
    let results = results_array(body, "/results", endpoint)?;
    Ok(envelopes_from_results(results, |item| {
        Some(ResultFields {
            // The result URL is `link`, not `url`, in Search1API's shape.
            url: item.get("link")?.as_str()?.to_owned(),
            title: item.get("title").and_then(Value::as_str).map(str::to_owned),
            // `content` holds a crawled full page body, populated only under
            // DeepSearch, which this adapter does not request; `snippet` is the
            // always-present search excerpt. Prefer a populated body, fall back
            // to the snippet, and leave the candidate textless rather than
            // inventing content when both are empty.
            text: item
                .get("content")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .or_else(|| {
                    item.get("snippet")
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty())
                })
                .map(str::to_owned),
            // The documented result shape carries no date field, so nothing
            // is mapped that the provider has never been seen to return.
            declared_date: None,
            promoted: &["link", "title", "snippet", "content"],
            ..Default::default()
        })
    }))
}

/// A `/crawl` carries no cost field either and bills separately from search
/// (one credit per crawled page), so its charge is quoted on its own basis.
fn quoted_crawl_charge() -> AcquisitionCharge {
    quoted_native(
        "credits",
        Number::from(1u64),
        "Search1API reports no cost on the crawl response; quoted from the published \
         one-credit-per-page crawl price.",
    )
}

/// Search1API reports no cost anywhere on the search response, so the only
/// honest figure is the published price, marked quoted. Recording it as
/// observed would be a fabrication, and recording nothing would lose the fact
/// that this search was billable.
///
/// The quoted figure is one credit, the published price of a plain search. A
/// DeepSearch (`crawl_results` > 0) would instead cost one credit plus one for
/// each page crawled — a variable amount this adapter does not incur, because
/// it does not crawl.
fn quoted_charge() -> AcquisitionCharge {
    quoted_native(
        "credits",
        Number::from(1u64),
        "Search1API reports no cost on the search response; quoted from the published \
         one-credit price of a plain search (crawl_results left at default, so no per-page \
         crawl credits are incurred).",
    )
}
