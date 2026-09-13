//! Keenable: open-web search over a pre-acquired page index, returning
//! per-result snippets.
//!
//! The verification state, its dated evidence and the per-operation basis
//! are `docs/knowledge-base/provider-verification.md`.
//!
//! The response shape is `results[{title,url,description,snippet}]`, with the
//! text taken from `snippet`. There is no cost field and no published flat
//! price, so the charge is recorded unknown. The wire format follows
//! Keenable's published HTTP API documentation — the OpenAPI
//! specification at `https://docs.keenable.ai/api-reference/openapi.json`, the
//! authentication guide at `https://docs.keenable.ai/authentication`, and the
//! credits guide at `https://docs.keenable.ai/credits` — all read this date. The
//! key is sent in the documented `X-API-Key` header; Keenable's keys carry a
//! `keen_` prefix and it rejects any other value as a malformed key. The test
//! beside this exercises the parser over the documented shapes through the real
//! transport and a loopback origin.
//!
//! Two wire facts shape this adapter and are worth stating plainly:
//!
//! - Keenable's `site` domain filter takes a *single* domain (a string), not a
//!   list. It is therefore only sent when the job names exactly one allowed
//!   host; a job naming several cannot be expressed in one request, so the
//!   search runs unscoped and admission enforces the operator's host policy
//!   over the results, exactly as for a provider with no filter at all.
//! - The search request has no documented result-count field. Rather than
//!   invent one, the adapter caps the *offered* candidates to the job's result
//!   limit itself and records that it did so; the sealed response still carries
//!   every result Keenable returned. When a count field is verified, move the
//!   cap onto the request so the provider trims before it counts the call.
//!
//! Keenable also documents `acquired_after`/`acquired_before`/`published_after`/
//! `published_before` date filters; this adapter sends none of them, because the
//! comparison run scopes freshness after admission, not at the supplier.

use commonmeasure_http::Request;
use commonmeasure_types::{AcquisitionCharge, ContextEnvelope, DeclaredDate, ProviderCapability};
use serde_json::{Value, json};

use crate::{
    Acquisition, ResultFields, SupplyAdapter, SupplyError, envelopes_from_results, execute,
    parse_json, results_array,
};

pub(crate) const DEFAULT_BASE_URL: &str = "https://api.keenable.ai";
/// No Keenable response carries a licence, rights or author field, so like the
/// other open-web providers this adapter declares search alone.
pub(crate) const CAPABILITIES: &[ProviderCapability] = &[ProviderCapability::Search];

/// Characters of snippet requested per result (`snippet_max_length`, documented
/// range 180–10000).
///
/// Fixed rather than derived from the job's context budget: a provider asked
/// for more text than another has been given an advantage the comparison did
/// not intend. Admission trims to the operator's budget afterwards. This mirrors
/// the fixed text cap the Exa adapter sends for the same reason.
const SNIPPET_MAX_CHARACTERS: u32 = 3000;

pub struct KeenableAdapter {
    base_url: String,
    api_key: String,
}

impl KeenableAdapter {
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

impl SupplyAdapter for KeenableAdapter {
    fn provider(&self) -> &str {
        "keenable"
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
        let mut body = json!({
            "query": query,
            "snippet_max_length": SNIPPET_MAX_CHARACTERS,
        });
        // `site` scopes the search to a single allowed source host. Keenable's
        // filter is single-valued, so it is sent only when the job named
        // exactly one host; a job naming several cannot be expressed here, and
        // sending the first of many would silently drop the others from the
        // search, so the request stays unscoped and admission enforces the host
        // policy over the results. Absent when the job named no hosts.
        if let [host] = include_hosts {
            body["site"] = json!(host);
        }
        let mut request = Request::post(
            "/",
            serde_json::to_vec(&body).map_err(|error| SupplyError::Malformed {
                detail: format!("could not serialise the request: {error}"),
            })?,
            "application/json",
        );
        // Keenable's documented authentication header. A keyless tier exists,
        // but the credential is sent whenever one is held so a metered account
        // is used when configured; an empty value simply carries no key.
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
            // The search response carries no request identifier (its only
            // top-level fields are `query` and `results`), so there is nothing
            // to join a provider-side receipt to.
            provider_request_id: None,
            charge: charge_from(&parsed),
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
    // holds every result Keenable returned. No Keenable response carries
    // licence metadata, so every envelope stays unknown.
    let results = results_array(body, "/results", endpoint)?
        .iter()
        .take(limit as usize);
    Ok(envelopes_from_results(results, |item| {
        Some(ResultFields {
            url: item.get("url")?.as_str()?.to_owned(),
            title: item.get("title").and_then(Value::as_str).map(str::to_owned),
            // `snippet` is the page-content excerpt Keenable returns, its length
            // governed by the `snippet_max_length` sent above. `description` is
            // the page's own meta description, a different field, and is left
            // in native metadata rather than treated as retrieved page text. An
            // empty or absent snippet leaves the candidate textless rather than
            // inventing content.
            text: item
                .get("snippet")
                .and_then(Value::as_str)
                .filter(|snippet| !snippet.is_empty())
                .map(str::to_owned),
            // `published_at` is Keenable's documented per-result publication
            // date. Mapped verbatim where present, with provenance naming the
            // field; a result without one stays undated rather than dated by a
            // neighbour. `acquired_at` is deliberately not used for this: it is
            // when Keenable indexed the page, a fact about the crawl and not
            // about when the content was published, so it stays in native
            // metadata.
            declared_date: item
                .get("published_at")
                .and_then(Value::as_str)
                .map(|date| DeclaredDate {
                    date: date.to_owned(),
                    provenance: "supplier-declared: published_at on the Keenable search result"
                        .to_owned(),
                }),
            promoted: &["url", "title", "snippet", "published_at"],
        })
    }))
}

/// Keenable's search response carries no cost field.
///
/// The documented `SearchResponse` has only `query` and `results`; billing is
/// per-organization with no published flat rate (the credits guide directs
/// callers to read the cost a call actually incurred rather than assume one),
/// and the unauthenticated tier is not metered at all. Over the MCP transport a
/// `_meta["keenable/usage"]` figure is documented, but this REST adapter never
/// sees it. So there is neither an observed charge to record nor a published
/// price to quote, and the charge is left unknown rather than recorded as zero —
/// an absent cost is not a free call.
fn charge_from(_body: &Value) -> AcquisitionCharge {
    AcquisitionCharge::default()
}
