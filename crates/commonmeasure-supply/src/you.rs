//! You.com: open-web search returning per-result excerpts.
//!
//! The verification state, its dated evidence and the per-operation basis
//! are `docs/knowledge-base/provider-verification.md`.
//!
//! The search response carries no cost field, so the charge is quoted from
//! the published per-call price. The wire format follows You.com's published
//! Search API documentation (the `POST /v1/search`
//! endpoint, `X-API-Key` auth, the `query`/`count`/`include_domains` request
//! body, and the `results.web`/`snippets`/`page_age` response with a
//! `metadata.search_uuid`), read on 18 August 2026 at
//! <https://you.com/docs/api-reference/search> (reached by following the
//! documented redirect from
//! <https://documentation.you.com/api-reference/search>). The tests beside
//! this exercise the parser over the documented shapes through the real
//! transport and a loopback origin.
//!
//! **Method choice.** You.com documents both a GET and a POST form of
//! `/v1/search` and states that the GET form "still works and existing
//! integrations will keep running, but it will not receive new feature
//! updates. New features will be added to POST only." The domain include filter
//! this experiment turns on — scoping a search to publishers that block AI
//! scraping — is a first-class body field on the POST form, so the adapter is
//! built to POST, which is the current, feature-complete surface, and sends the
//! filter as a JSON array exactly as the other open-web adapters do.
//!
//! **Fields deliberately not sent.** The documented request also accepts
//! `exclude_domains`, `boost_domains`, `freshness`, `offset`, `country`,
//! `language`, `safesearch` and an `extraction` object. None is sent: the run
//! names only hosts to include, and requesting `extraction` (full-page content)
//! would add a separately-priced per-page charge and put this plan's cost on a
//! different basis from the other search-only plans in the run. The response
//! also splits results into `results.web` and `results.news`; only `web` is
//! read, because merging two provider-ranked lists would invent a combined rank
//! You.com never produced. Both untouched shapes still survive whole in the
//! sealed raw response.

use commonmeasure_http::Request;
use commonmeasure_types::{AcquisitionCharge, ContextEnvelope, DeclaredDate, ProviderCapability};
use serde_json::{Number, Value, json};

use crate::{
    Acquisition, ResultFields, SupplyAdapter, SupplyError, envelopes_from_results, execute,
    parse_json, quoted_native, results_array,
};

pub(crate) const DEFAULT_BASE_URL: &str = "https://ydc-index.io";
/// No You.com response carries a licence, rights or author field, so like the
/// other open-web providers this adapter declares search alone.
pub(crate) const CAPABILITIES: &[ProviderCapability] = &[ProviderCapability::Search];

pub struct YouAdapter {
    base_url: String,
    api_key: String,
}

impl YouAdapter {
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

impl SupplyAdapter for YouAdapter {
    fn provider(&self) -> &str {
        "you"
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
        // `count` is You.com's documented per-section result cap, so it is set
        // to the job's result limit and the provider trims web results before
        // billing — there is no client-side cap here, unlike the providers
        // whose count field is unwired.
        let mut body = json!({
            "query": query,
            "count": limit,
        });
        // `include_domains` scopes the search to the operator's allowed source
        // hosts. Absent when the job named none, so an unscoped search sends the
        // body it always did; present, it asks You.com for only the hosts
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
        request.headers.set("X-API-Key", &self.api_key);
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
                .pointer("/metadata/search_uuid")
                .and_then(Value::as_str)
                .map(str::to_owned),
            charge: quoted_charge(),
            envelopes,
            raw_response: response.body,
            invocation: None,
        })
    }
}

/// No You.com response carries licence metadata, so every envelope stays
/// unknown.
fn envelopes_from(body: &Value, endpoint: &str) -> Result<Vec<ContextEnvelope>, SupplyError> {
    let results = results_array(body, "/results/web", endpoint)?;
    Ok(envelopes_from_results(results, |item| {
        Some(ResultFields {
            url: item.get("url")?.as_str()?.to_owned(),
            title: item.get("title").and_then(Value::as_str).map(str::to_owned),
            // You.com returns page text as `snippets`, an array of short
            // fragments taken from the result page. They are joined with blank
            // lines into the one text field the envelope carries; an empty or
            // absent array leaves the candidate textless rather than inventing
            // content. The `description` field is a summary, not page text, so
            // it stays in native metadata rather than being promoted as the
            // page's own words.
            text: item
                .get("snippets")
                .and_then(Value::as_array)
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n\n")
                })
                .filter(|joined| !joined.is_empty()),
            // `page_age` is You.com's documented per-result publication
            // timestamp. Mapped verbatim where present, with provenance naming
            // the field; a result without one stays undated rather than dated
            // by a neighbour. It is the supplier's claim about the page, never
            // verified against the content.
            declared_date: item
                .get("page_age")
                .and_then(Value::as_str)
                .filter(|date| !date.is_empty())
                .map(|date| DeclaredDate {
                    date: date.to_owned(),
                    provenance: "supplier-declared: page_age on the You.com search result"
                        .to_owned(),
                }),
            promoted: &["url", "title", "snippets", "page_age"],
        })
    }))
}

/// You.com reports no charge on the search response — `metadata` carries only
/// the echoed query, a `search_uuid` and a latency in seconds — so the only
/// honest figure is the published price, marked quoted.
///
/// The published price is stated in currency (USD 5.00 per 1,000 search calls,
/// i.e. USD 0.005 per call), but `money` is reserved for a currency charge the
/// provider *reported on this response* (`commonmeasure_types::AcquisitionCharge`), and
/// nothing was. The quoted price therefore rides the native charge alone, in
/// the provider's own unit with basis `Quoted`; putting it in `money` would let
/// a not-yet-billed figure read as an observed receipt, and recording nothing
/// would lose the fact that this search is billable.
fn quoted_charge() -> AcquisitionCharge {
    // USD 5.00 per 1,000 calls is USD 0.005 per call. Parsed from the decimal
    // literal so the recorded amount keeps the price's exact text.
    let amount: Number = "0.005"
        .parse()
        .expect("a decimal price literal is a valid JSON number");
    quoted_native(
        "USD",
        amount,
        "You.com reports no cost on the search response; quoted from the published price of USD \
         0.005 per search call (USD 5.00 per 1,000). Extraction, which this adapter does not \
         request, is priced separately.",
    )
}
