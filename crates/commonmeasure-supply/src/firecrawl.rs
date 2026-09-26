//! Firecrawl: open-web search, and scrape as the `fetch` capability.
//!
//! `search` returns snippets; `fetch` scrapes a named URL to its full page
//! markdown via `/v2/scrape`. Firecrawl charges in credits, never currency.

use commonmeasure_http::Request;
use commonmeasure_types::canonical::sha256_digest;
use commonmeasure_types::{
    AcquisitionCharge, ChargeBasis, ContextEnvelope, LicenceState, NativeCharge, ProviderCapability,
};
use serde_json::{Value, json};

use crate::{
    Acquisition, ResultFields, SupplyAdapter, SupplyError, envelopes_from_results, execute,
    host_of, native_metadata, parse_json, required_field, results_array,
};

pub(crate) const DEFAULT_BASE_URL: &str = "https://api.firecrawl.dev";
/// Search discovers candidates (returning snippets); fetch scrapes a named URL
/// to its full page markdown through `/v2/scrape`.
pub(crate) const CAPABILITIES: &[ProviderCapability] =
    &[ProviderCapability::Search, ProviderCapability::Fetch];

pub struct FirecrawlAdapter {
    base_url: String,
    api_key: String,
}

impl FirecrawlAdapter {
    pub fn new(base_url: &str, api_key: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key: api_key.to_owned(),
        }
    }
}

impl SupplyAdapter for FirecrawlAdapter {
    fn provider(&self) -> &str {
        "firecrawl"
    }

    fn capabilities(&self) -> &'static [ProviderCapability] {
        CAPABILITIES
    }

    // `include_hosts` is ignored: this adapter does not wire a Firecrawl
    // domain filter, so the search is unscoped and admission enforces the
    // operator's host policy over the results, exactly as before.
    fn search(
        &self,
        query: &str,
        limit: u32,
        _include_hosts: &[&str],
    ) -> Result<Acquisition, SupplyError> {
        let endpoint = format!("{}/v2/search", self.base_url);
        // Search only. `scrapeOptions` would spend scrape credits per result
        // on top of the search charge, which would make this plan's cost
        // incomparable with the other search-only plans in the same run.
        let body = json!({"query": query, "limit": limit, "sources": ["web"]});
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
            provider_request_id: parsed.get("id").and_then(Value::as_str).map(str::to_owned),
            charge: charge_from(&parsed),
            envelopes,
            raw_response: response.body,
            invocation: None,
        })
    }

    fn fetch(&self, url: &str) -> Result<Acquisition, SupplyError> {
        let endpoint = format!("{}/v2/scrape", self.base_url);
        // Markdown only. A scrape returns the full page body, which is the
        // point of a fetch; other formats would add separately-priced work this
        // acquisition did not ask for.
        let body = json!({"url": url, "formats": ["markdown"]});
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
        let envelopes = scrape_envelope(&parsed, url, &endpoint)?;

        Ok(Acquisition {
            provider: self.provider().to_owned(),
            capability: ProviderCapability::Fetch,
            endpoint,
            http_status: Some(response.status),
            latency_ms,
            provider_request_id: parsed
                .pointer("/data/metadata/scrapeId")
                .and_then(Value::as_str)
                .map(str::to_owned),
            charge: charge_from(&parsed),
            envelopes,
            raw_response: response.body,
            invocation: None,
        })
    }
}

/// The one envelope a scrape produces: the page markdown as text, keyed to the
/// URL Firecrawl reports it scraped (`data.metadata.sourceURL`), falling back to
/// the requested URL. Empty markdown yields no envelope rather than a textless
/// source. Metadata other than the promoted fields is kept namespaced.
fn scrape_envelope(
    body: &Value,
    requested: &str,
    endpoint: &str,
) -> Result<Vec<ContextEnvelope>, SupplyError> {
    let data = required_field(body, "/data", endpoint)?;
    let source_url = data
        .pointer("/metadata/sourceURL")
        .and_then(Value::as_str)
        .unwrap_or(requested)
        .to_owned();
    let Some(host) = host_of(&source_url) else {
        return Ok(Vec::new());
    };
    // An absent `markdown` is a scrape that returned no page at all, and
    // there is nothing to admit. An empty one is a page the scraper read as
    // empty, which is a fact about the page and is admitted as such
    // (`commonmeasure_types::ContextEnvelope::text`).
    let Some(text) = data
        .get("markdown")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return Ok(Vec::new());
    };
    Ok(vec![ContextEnvelope {
        host,
        content_hash: Some(sha256_digest(text.as_bytes())),
        title: data
            .pointer("/metadata/title")
            .and_then(Value::as_str)
            .map(str::to_owned),
        text: Some(text),
        // Rich page metadata, no rights field on any endpoint tested.
        licence: LicenceState::Unknown,
        // The scrape metadata carries no reliable publication date; `cachedAt`
        // is a fact about Firecrawl's cache, not the content, so none is mapped.
        declared_date: None,
        native_metadata: native_metadata(data, &["markdown"]),
        retrieval_rank: 1,
        source_url,
    }])
}

/// Rich page metadata, no rights field on any endpoint tested, so every
/// envelope stays unknown.
fn envelopes_from(body: &Value, endpoint: &str) -> Result<Vec<ContextEnvelope>, SupplyError> {
    let results = results_array(body, "/data/web", endpoint)?;
    Ok(envelopes_from_results(results, |item| {
        Some(ResultFields {
            url: item.get("url")?.as_str()?.to_owned(),
            title: item.get("title").and_then(Value::as_str).map(str::to_owned),
            // A search result carries `description`, not page text. Calling a
            // snippet full-page extraction is exactly the equivalence
            // `docs/contracts/experiment.md` forbids, so it is admitted as the
            // excerpt it is and the native metadata keeps the field name.
            text: item
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned),
            // No date field on any recorded search result
            // and none documented for the search
            // endpoint. The scrape leg's `cachedAt` is a fact about Firecrawl's
            // cache, not about the content, and would be a fabricated
            // publication date here.
            declared_date: None,
            promoted: &["url", "title", "description"],
            ..Default::default()
        })
    }))
}

/// Credits, never currency, and the field moves between endpoints: top level
/// on `/v2/search`, nested at `data.metadata.creditsUsed` on `/v2/scrape`.
/// No USD figure is returned and the conversion depends on plan, so any dollar
/// figure for Firecrawl would be derived rather than observed and is not
/// produced here.
fn charge_from(body: &Value) -> AcquisitionCharge {
    let Some(credits) = body
        .get("creditsUsed")
        .or_else(|| body.pointer("/data/metadata/creditsUsed"))
        .and_then(Value::as_number)
    else {
        return AcquisitionCharge::default();
    };
    AcquisitionCharge {
        money: None,
        native: Some(NativeCharge {
            unit: "credits".to_owned(),
            amount: credits.clone(),
            basis: ChargeBasis::Observed,
            note: None,
        }),
    }
}
