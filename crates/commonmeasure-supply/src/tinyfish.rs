//! TinyFish: open-web search returning ranked results with per-result
//! snippets.
//!
//! The wire format here is built to TinyFish's published Search API reference
//! (`https://docs.tinyfish.ai/search-api/reference.md`). The tests beside this
//! exercise the parser over the documented shapes through the real transport
//! and a loopback origin.
//!
//! A supply-class note before the wire facts. TinyFish sells four API surfaces
//! — Search, Fetch, Agent and Browser — and markets the latter two on stealth
//! sessions that navigate login walls and defeat anti-bot protections, with a
//! credential vault and persistent logged-in browser profiles. This adapter
//! wires the Search API alone and none of those capabilities, because they
//! execute crossings a site has already refused. "Crawler accessibility is not
//! permission" applies to this supplier with more than the usual force, and
//! every result stays `LicenceState::Unknown` accordingly.
//!
//! Wire facts that shape this adapter:
//!
//! - Search is a `GET` on its own host (`https://api.search.tinyfish.ai`) with
//!   parameters in the query string and the key in the documented `X-API-Key`
//!   header — unlike the JSON-POST search providers in this crate.
//! - `include_domains` is a comma-separated list, so the job's allowed source
//!   hosts can all be expressed in one request; it is sent only when the job
//!   names hosts, and admission still enforces the host policy over the
//!   results either way.
//! - The request has no documented result-count field. Rather than invent one,
//!   the adapter caps the *offered* candidates to the job's result limit
//!   itself; the sealed response still carries every result TinyFish returned.
//! - The optional `purpose` parameter — a statement of why the operator is
//!   searching, traded for "better-quality results" — is deliberately never
//!   sent. It would disclose the job's intent to the supplier and change what
//!   a search consumes, a trade the comparison must not make silently.
//! - `domain_type` is left to its documented default (`web`). The `news` and
//!   `research_paper` variants return extra per-result fields (`publisher`,
//!   `authors`, `venue`, `year`); any that ever arrive on a web search stay in
//!   native metadata.
//! - The response carries no cost, credit or token field and no request
//!   identifier, so `provider_request_id` stays `None` and the charge is
//!   quoted from the published price — which is zero: "Search requests are
//!   free at any wallet balance, including $0", though the account still needs
//!   Search API access (`402` otherwise). A published zero is a price, not an
//!   absent cost, so it is recorded `Quoted` rather than left unknown.

use commonmeasure_http::Request;
use commonmeasure_types::{AcquisitionCharge, ContextEnvelope, DeclaredDate, ProviderCapability};
use serde_json::Value;

use crate::{
    Acquisition, ResultFields, SupplyAdapter, SupplyError, envelopes_from_results, execute,
    parse_json, quoted_money, results_array, urlencode,
};

pub(crate) const DEFAULT_BASE_URL: &str = "https://api.search.tinyfish.ai";
/// Search alone: no TinyFish search response carries a licence, rights or
/// author field, so like the other open-web providers this adapter declares
/// search. Fetch, Agent and Browser exist but are not wired (see the module
/// note on supply class).
pub(crate) const CAPABILITIES: &[ProviderCapability] = &[ProviderCapability::Search];

/// The published price of a search request: free at any wallet balance
/// (`https://docs.tinyfish.ai/search-api/reference.md` §Billing, read
/// 25 August 2026).
const QUOTED_PRICE_USD: &str = "0";

pub struct TinyfishAdapter {
    base_url: String,
    api_key: String,
}

impl TinyfishAdapter {
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

impl SupplyAdapter for TinyfishAdapter {
    fn provider(&self) -> &str {
        "tinyfish"
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
        // Parameters travel in the query string: TinyFish's search is a GET on
        // its own host, not a JSON POST.
        let mut endpoint = format!("{}/?query={}", self.base_url, urlencode(query));
        // `include_domains` takes the whole allowed-host set as a
        // comma-separated list (the documented form uses literal commas, e.g.
        // `github.com,arxiv.org`). Sent only when the job named hosts, so an
        // ordinary open-web run is byte-identical to an unscoped one; admission
        // enforces the operator's host policy over the results either way.
        if !include_hosts.is_empty() {
            let domains: Vec<String> = include_hosts.iter().map(|host| urlencode(host)).collect();
            endpoint.push_str("&include_domains=");
            endpoint.push_str(&domains.join(","));
        }
        let mut request = Request::get("/");
        request.headers.set("X-API-Key", &self.api_key);
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
            // The documented response has only `query`, `results`,
            // `total_results` and `page` at the top level — no request or trace
            // identifier anywhere, so there is nothing to join a provider-side
            // receipt to.
            provider_request_id: None,
            charge: quoted_charge(),
            envelopes,
            raw_response: response.body,
            invocation: None,
        })
    }
}

fn envelopes_from(
    body: &Value,
    limit: u32,
    endpoint: &str,
) -> Result<Vec<ContextEnvelope>, SupplyError> {
    // The request carries no result-count field (see the module note), so the
    // adapter honours the job's result limit here. The sealed response still
    // holds every result TinyFish returned. No TinyFish response carries
    // licence metadata, so every envelope stays unknown.
    let results = results_array(body, "/results", endpoint)?
        .iter()
        .take(limit as usize);
    Ok(envelopes_from_results(results, |item| {
        Some(ResultFields {
            url: item.get("url")?.as_str()?.to_owned(),
            title: item.get("title").and_then(Value::as_str).map(str::to_owned),
            // `snippet` is the only page text a search result carries. An empty
            // or absent one leaves the candidate textless rather than inventing
            // content.
            text: item
                .get("snippet")
                .and_then(Value::as_str)
                .filter(|snippet| !snippet.is_empty())
                .map(str::to_owned),
            // `date` is TinyFish's documented per-result publication date,
            // "present for news results and some web results". Mapped verbatim
            // where present, with provenance naming the field; a result without
            // one stays undated rather than dated by a neighbour.
            declared_date: item
                .get("date")
                .and_then(Value::as_str)
                .filter(|date| !date.is_empty())
                .map(|date| DeclaredDate {
                    date: date.to_owned(),
                    provenance: "supplier-declared: date on the TinyFish search result".to_owned(),
                }),
            // Keeps `position` (TinyFish's own 1-indexed rank, which agrees
            // with the offered order unless the provider ever reorders),
            // `site_name`, and any news/academic extras.
            promoted: &["url", "title", "snippet", "date"],
            ..Default::default()
        })
    }))
}

/// TinyFish reports no charge anywhere on the search response, and its
/// published price for a search request is zero: "free at any wallet balance,
/// including $0". A published zero is a price, not an absent cost, so both the
/// currency amount and the native figure are recorded, marked quoted — never
/// observed, and never left unknown, which would hide that the supplier has
/// priced this operation at nothing.
fn quoted_charge() -> AcquisitionCharge {
    quoted_money(
        "USD",
        QUOTED_PRICE_USD,
        "TinyFish reports no cost field on the search response; quoted from the published price \
         — search requests are free at any wallet balance (Search API reference, read 25 August \
         2026).",
    )
}
