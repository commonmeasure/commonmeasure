//! SERPdive: open-web search returning extracted page content.
//!
//! The wire format follows SERPdive's published API
//! documentation (`https://serpdive.com/docs`, read 18 August 2026):
//! `POST /v1/search` with `Authorization: Bearer` auth, a `query`/`model`/
//! `max_results` request, and a `results` array of `{url, title, content,
//! date}`. The tests beside this exercise the parser over the documented shapes
//! through the real transport and a loopback origin.
//!
//! SERPdive documents no domain include/allow filter, so the search cannot be
//! scoped provider-side and `include_hosts` is ignored here; admission still
//! enforces the operator's host policy over the returned sources, exactly as it
//! does for the other unfilterable providers. The response carries no request
//! identifier and no cost, credit or token field anywhere in its body, so none
//! is read: `provider_request_id` stays `None`, and the charge is quoted from
//! the model's published credit price rather than observed. The opt-in `answer`
//! field is deliberately not requested — this run wants sources to compare, not
//! a written answer, and asking for one would change what a search consumes.
//!
//! The `mako` model is selected explicitly rather than left to default: it is
//! the documented default, returns extracted page content, and bills a flat one
//! credit, which keeps this plan on the same one-credit basis as the other
//! metered search providers in the run. `krill` (free) and `moby` (1.5 credits,
//! full page content) are the other documented models.

use commonmeasure_http::Request;
use commonmeasure_types::{AcquisitionCharge, ContextEnvelope, DeclaredDate, ProviderCapability};
use serde_json::{Number, Value, json};

use crate::{
    Acquisition, ResultFields, SupplyAdapter, SupplyError, envelopes_from_results, execute,
    parse_json, quoted_native, results_array,
};

pub(crate) const DEFAULT_BASE_URL: &str = "https://api.serpdive.com";
/// Search alone: no SERPdive response carries a licence, rights or author
/// field, so like the other open-web providers this adapter declares search.
pub(crate) const CAPABILITIES: &[ProviderCapability] = &[ProviderCapability::Search];

/// SERPdive's `max_results` is documented as an integer in the range 1–10, so
/// the job's limit is clamped into it. The provider trims to this count itself,
/// which is why the offered candidates need no separate cap.
const MAX_RESULTS: u32 = 10;

/// The model this adapter requests. `mako` is SERPdive's documented default,
/// returns extracted page content, and bills one credit; selecting it
/// explicitly keeps the request deterministic and comparable across runs.
const SEARCH_MODEL: &str = "mako";

pub struct SerpdiveAdapter {
    base_url: String,
    api_key: String,
}

impl SerpdiveAdapter {
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

impl SupplyAdapter for SerpdiveAdapter {
    fn provider(&self) -> &str {
        "serpdive"
    }

    fn capabilities(&self) -> &'static [ProviderCapability] {
        CAPABILITIES
    }

    // `include_hosts` is ignored: SERPdive documents no domain include filter,
    // so the search is unscoped and admission enforces the operator's host
    // policy over the returned sources, exactly as before.
    fn search(
        &self,
        query: &str,
        limit: u32,
        _include_hosts: &[&str],
    ) -> Result<Acquisition, SupplyError> {
        let endpoint = format!("{}/v1/search", self.base_url);
        // `max_results` is documented as 1–10; the job's limit is clamped into
        // that window so the request never asks for a count the provider would
        // reject. The provider trims to this count itself.
        let max_results = limit.clamp(1, MAX_RESULTS);
        let body = json!({
            "query": query,
            "model": SEARCH_MODEL,
            "max_results": max_results,
        });
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
            // SERPdive returns no request or trace identifier in the response
            // body or headers, so there is nothing to join a provider-side
            // receipt to.
            provider_request_id: None,
            charge: quoted_charge(),
            envelopes,
            raw_response: response.body,
            invocation: None,
        })
    }
}

/// No SERPdive response carries licence metadata, so every envelope stays
/// unknown.
fn envelopes_from(body: &Value, endpoint: &str) -> Result<Vec<ContextEnvelope>, SupplyError> {
    let results = results_array(body, "/results", endpoint)?;
    Ok(envelopes_from_results(results, |item| {
        Some(ResultFields {
            url: item.get("url")?.as_str()?.to_owned(),
            title: item.get("title").and_then(Value::as_str).map(str::to_owned),
            // SERPdive returns page text as `content`, a single string. An
            // empty or absent one leaves the candidate textless rather than
            // inventing content.
            text: item
                .get("content")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_owned),
            // `date` is SERPdive's documented per-result publication date, ISO
            // `YYYY-MM-DD` and absent when unknown. Mapped verbatim where
            // present, with provenance naming the field; a result without one
            // stays undated rather than dated by a neighbour.
            declared_date: item
                .get("date")
                .and_then(Value::as_str)
                .filter(|date| !date.is_empty())
                .map(|date| DeclaredDate {
                    date: date.to_owned(),
                    provenance: "supplier-declared: date on the SERPdive search result".to_owned(),
                }),
            promoted: &["url", "title", "content", "date"],
            ..Default::default()
        })
    }))
}

/// SERPdive reports no charge anywhere on the search response — no cost, credit
/// or token field appears in its body — so the only honest figure is the
/// published price for the requested model, marked quoted. Recording it as
/// observed would be a fabrication, and recording nothing would lose the fact
/// that this search was billable.
fn quoted_charge() -> AcquisitionCharge {
    // No currency is disclosed: billing is credit-based and the credit-to-
    // dollar conversion depends on the account's plan, so a dollar figure here
    // would be derived, not observed. The published `mako` price is one credit
    // per search.
    quoted_native(
        "credits",
        Number::from(1u64),
        "SERPdive reports no cost, credit or token field on the search response; quoted from \
         the published one-credit price of the mako model this adapter requests.",
    )
}
