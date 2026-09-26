//! Linkup: open-web search returning ranked URLs and content snippets.
//!
//! Neither the search nor the fetch response carries a cost field. The wire
//! format follows Linkup's published Search API documentation (the
//! `POST /v1/search` endpoint,
//! `Authorization: Bearer` auth, the `q`/`depth`/`outputType`/`maxResults`
//! request, the `includeDomains` domain filter, and the `searchResults`
//! response of a `results` array of `{type, name, url, content, favicon}`),
//! read on 18 August 2026 at
//! <https://docs.linkup.so/pages/documentation/api-reference/endpoint/post-search>.
//! The tests beside this exercise the parser over the documented shapes through
//! the real transport and a loopback origin.
//!
//! Linkup discloses no cost, credit or usage field anywhere on the search
//! response, so the only honest figure is the published price, marked
//! `Quoted`: a `standard`-depth `searchResults` call is billed at $0.005 per
//! call. Recording it as `Observed` would be a fabrication, and recording
//! nothing would lose the fact that this search was billable. A `/v1/fetch`
//! response carries no cost field either, and no per-fetch price has been read
//! from Linkup's published pricing, so the fetch charge is left unknown: there
//! is nothing observed and nothing quoted, and an absent cost is not a free
//! call.
//!
//! Linkup returns no per-result publication date and no request identifier on
//! the `searchResults` output, so this adapter maps neither: it never invents
//! a field the documentation does not locate on the response.

use commonmeasure_http::Request;
use commonmeasure_types::canonical::sha256_digest;
use commonmeasure_types::{AcquisitionCharge, ContextEnvelope, LicenceState, ProviderCapability};
use serde_json::{Value, json};

use crate::{
    Acquisition, ResultFields, SupplyAdapter, SupplyError, envelopes_from_results, execute,
    host_of, native_metadata, parse_json, quoted_money, required_field, results_array,
};

pub(crate) const DEFAULT_BASE_URL: &str = "https://api.linkup.so";
/// Search discovers candidates; fetch retrieves a named URL through `/v1/fetch`,
/// which returns clean page markdown. No Linkup response carries a licence.
pub(crate) const CAPABILITIES: &[ProviderCapability] =
    &[ProviderCapability::Search, ProviderCapability::Fetch];

/// The published per-call price of a `standard`-depth `searchResults` search,
/// in USD, read from Linkup's pricing on 18 August 2026. Kept as decimal text
/// so a price that is already exact never acquires a rounding error on its way
/// into the evidence record.
const QUOTED_PRICE_USD: &str = "0.005";

pub struct LinkupAdapter {
    base_url: String,
    api_key: String,
}

impl LinkupAdapter {
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

impl SupplyAdapter for LinkupAdapter {
    fn provider(&self) -> &str {
        "linkup"
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
        // `searchResults` returns ranked URLs with content snippets for
        // grounding; `sourcedAnswer` and `structured` would have Linkup compose
        // an answer, which is inference this comparison performs itself, not
        // supply. `standard` depth is the cheapest tier that returns content
        // ($0.005); `deep` costs ten times as much and would put this plan on a
        // different price basis from the rest of the run. `maxResults` is the
        // documented server-side count, so Linkup trims before it answers and
        // no offered-candidate cap is needed here.
        let mut body = json!({
            "q": query,
            "depth": "standard",
            "outputType": "searchResults",
            "maxResults": limit,
        });
        // `includeDomains` scopes the search to the operator's allowed source
        // hosts (a whitelist of up to 100 domains). Absent when the job named
        // none, so an unscoped search sends the body it always did; present, it
        // asks Linkup for only the hosts admission would keep anyway.
        if !include_hosts.is_empty() {
            body["includeDomains"] = json!(include_hosts);
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
            // No request identifier appears anywhere on the documented
            // `searchResults` response, so there is nothing to join a
            // provider-side receipt to.
            provider_request_id: None,
            charge: quoted_charge(),
            envelopes,
            raw_response: response.body,
            invocation: None,
        })
    }

    fn fetch(&self, url: &str) -> Result<Acquisition, SupplyError> {
        let endpoint = format!("{}/v1/fetch", self.base_url);
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
        let envelopes = fetch_envelope(&parsed, url, &endpoint)?;

        Ok(Acquisition {
            provider: self.provider().to_owned(),
            capability: ProviderCapability::Fetch,
            endpoint,
            http_status: Some(response.status),
            latency_ms,
            provider_request_id: None,
            charge: unknown_fetch_charge(),
            envelopes,
            raw_response: response.body,
            invocation: None,
        })
    }
}

/// The one envelope a `/v1/fetch` produces: the page body as `markdown`. The
/// response carries no URL of its own, so the source is the URL that was
/// requested. Empty markdown yields no envelope rather than a textless source.
fn fetch_envelope(
    body: &Value,
    requested: &str,
    endpoint: &str,
) -> Result<Vec<ContextEnvelope>, SupplyError> {
    let text = required_field(body, "/markdown", endpoint)?
        .as_str()
        .filter(|markdown| !markdown.is_empty())
        .map(str::to_owned);
    let Some(text) = text else {
        return Ok(Vec::new());
    };
    let Some(host) = host_of(requested) else {
        return Ok(Vec::new());
    };
    Ok(vec![ContextEnvelope {
        host,
        content_hash: Some(sha256_digest(text.as_bytes())),
        // The fetch response carries no title; naming one would be invention.
        title: None,
        text: Some(text),
        licence: LicenceState::Unknown,
        declared_date: None,
        native_metadata: native_metadata(body, &["markdown"]),
        retrieval_rank: 1,
        source_url: requested.to_owned(),
    }])
}

/// No Linkup response carries licence metadata, so every envelope stays
/// unknown.
fn envelopes_from(body: &Value, endpoint: &str) -> Result<Vec<ContextEnvelope>, SupplyError> {
    let results = results_array(body, "/results", endpoint)?;
    Ok(envelopes_from_results(results, |item| {
        Some(ResultFields {
            url: item.get("url")?.as_str()?.to_owned(),
            // Linkup names the result title `name`, not `title`.
            title: item.get("name").and_then(Value::as_str).map(str::to_owned),
            // `content` is the snippet Linkup returns per result; an absent or
            // empty one leaves the candidate textless rather than inventing
            // content. `type` distinguishes text from image results and stays
            // in native metadata.
            text: item
                .get("content")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_owned),
            // Linkup returns no per-result date on the `searchResults` output
            // (date filtering is a request parameter, `fromDate`/`toDate`, not
            // a response field), so nothing is mapped that has never been seen.
            declared_date: None,
            promoted: &["url", "name", "content"],
            ..Default::default()
        })
    }))
}

/// Linkup's fetch response carries no cost field.
///
/// The documented `/v1/fetch` response is `favicon` and `markdown` alone, and it
/// bills separately from search on a basis whose published price this operator
/// has not read. So there is neither an observed charge to record nor a
/// published price to quote, and the charge is left unknown rather than recorded
/// as zero or as an invented unit — an absent cost is not a free call, and
/// nothing was quoted.
fn unknown_fetch_charge() -> AcquisitionCharge {
    AcquisitionCharge::default()
}

/// Linkup reports no charge anywhere on the search response, so the only honest
/// figure is the published price, marked quoted. It is disclosed in currency,
/// so both the comparable USD amount and the native figure are recorded;
/// recording it as observed would be a fabrication, and recording nothing would
/// lose the fact that this search was billable.
fn quoted_charge() -> AcquisitionCharge {
    quoted_money(
        "USD",
        QUOTED_PRICE_USD,
        "Linkup reports no cost on the search response; quoted from the published \
         standard-depth searchResults price of 0.005 USD per call.",
    )
}
