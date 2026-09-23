//! Nimble: open-web search returning page content.
//!
//! The search response carries no cost, credit or usage field. The wire
//! format follows Nimble's published Web API search documentation (the
//! `POST /v2/search`
//! endpoint, `Authorization: Bearer` auth, the `query`/`max_results`/
//! `search_depth` request, the `include_domains` domain filter, and the
//! `results`/`content`/`request_id` response), read this date at
//! <https://docs.nimbleway.com/nimble-sdk/web-tools/search>,
//! <https://docs.nimbleway.com/api-reference/search/search.md> and
//! <https://docs.nimbleway.com/nimble-sdk/web-tools/search-depth.md>. First live
//! contact showed the per-result `content` arriving empty on a plain search
//! while `description` carried the SERP snippet, so the parser falls back to the
//! snippet (below). The tests exercise both paths over the documented shapes
//! through the real transport and a loopback origin.
//!
//! Two things the documentation does not give are handled by not inventing
//! them. Nimble documents no per-result published-date field on the search
//! response, so the adapter maps no date at all rather than guessing a field
//! name. And the search response carries no cost, credit or usage field
//! anywhere in its schema, so the charge is quoted from the published
//! per-search price of the depth this adapter requests, marked `Quoted`, never
//! recorded as observed or as zero. The many optional request fields Nimble
//! offers — `exclude_domains`, `focus`, `content_type`, `country`, `locale`,
//! `time_range`, `start_date`, `end_date`, `max_subagents`, `output_format` —
//! are deliberately not sent: none is needed to run a plain open-web search,
//! and sending a value the comparison did not intend would give this provider
//! terms the others were not asked for.

use commonmeasure_http::Request;
use commonmeasure_types::{AcquisitionCharge, ContextEnvelope, ProviderCapability};
use serde_json::{Number, Value, json};

use crate::{
    Acquisition, ResultFields, SupplyAdapter, SupplyError, envelopes_from_results, execute,
    parse_json, quoted_native, results_array,
};

pub(crate) const DEFAULT_BASE_URL: &str = "https://sdk.nimbleway.com";
/// No Nimble search response carries a licence, rights or author field, so like
/// the other open-web providers this adapter declares search alone.
pub(crate) const CAPABILITIES: &[ProviderCapability] = &[ProviderCapability::Search];

/// The documented ceiling on `max_results` (`1..=100`, default 10). The job's
/// result limit is clamped to it so the count travels on the request and Nimble
/// trims server-side; there is no need to cap the offered candidates afterwards.
const MAX_RESULTS: u32 = 100;

/// The search depth this adapter requests.
///
/// `fast` (Nimble's own default) returns the rich per-result `content` this
/// comparison needs, at a flat two-credit-per-search price, which keeps this
/// plan on a single, comparable price basis with the other open-web providers
/// in the same run. `lite` returns only titles, URLs and snippets — no page
/// content — and `deep` bills per extracted page (`1 + 1 per page`), a variable
/// basis that would not compare like-for-like against a flat per-search charge.
const SEARCH_DEPTH: &str = "fast";

pub struct NimbleAdapter {
    base_url: String,
    api_key: String,
}

impl NimbleAdapter {
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

impl SupplyAdapter for NimbleAdapter {
    fn provider(&self) -> &str {
        "nimble"
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
        let endpoint = format!("{}/v2/search", self.base_url);
        // The result count travels on the request, clamped to Nimble's
        // documented ceiling, so the provider trims before it answers and the
        // adapter does not have to cap the offered candidates itself.
        let mut body = json!({
            "query": query,
            "max_results": limit.clamp(1, MAX_RESULTS),
            "search_depth": SEARCH_DEPTH,
        });
        // `include_domains` scopes the search to the operator's allowed source
        // hosts. Absent when the job named none, so an unscoped search sends the
        // body it always did; present, it asks Nimble for only the hosts
        // admission would keep anyway. Nimble accepts up to 50 domains here; the
        // operator's allow list is well within that, so no truncation is needed.
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
}

/// No Nimble search response carries licence metadata, so every envelope
/// stays unknown.
fn envelopes_from(body: &Value, endpoint: &str) -> Result<Vec<ContextEnvelope>, SupplyError> {
    let results = results_array(body, "/results", endpoint)?;
    Ok(envelopes_from_results(results, |item| {
        // `content` is Nimble's extracted page text, but on a plain search it
        // frequently arrives empty while `description` carries the SERP
        // snippet. An empty body is no body: the adapter falls back to the
        // snippet — the same excerpt the other search providers admit — rather
        // than reporting a textless result beside content the provider did
        // surface. Whichever is used, the other field survives whole in native
        // metadata.
        let text = item
            .get("content")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .or_else(|| {
                item.get("description")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
            })
            .map(str::to_owned);
        // Whether the snippet became the text, so it can be promoted out of
        // native metadata below.
        let used_description = text.is_some()
            && item
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .is_empty();
        // `metadata` (position, entity_type, country, locale) always stays
        // namespaced. `description` (the SERP snippet) stays too, except on a
        // result where it became the envelope text — then it is promoted out
        // so the one fact does not appear twice under two names.
        let promoted: &'static [&'static str] = if used_description {
            &["url", "title", "content", "description"]
        } else {
            &["url", "title", "content"]
        };
        Some(ResultFields {
            url: item.get("url")?.as_str()?.to_owned(),
            title: item.get("title").and_then(Value::as_str).map(str::to_owned),
            text,
            // Nimble documents no per-result published-date field on the
            // search response, so nothing is mapped that has never been seen.
            // A date guessed at a field name would be exactly the fabricated
            // measurement the freshness evaluator exists to refuse.
            declared_date: None,
            promoted,
        })
    }))
}

/// Nimble's search response carries no cost, credit or usage field anywhere in
/// its documented schema, so the only honest figure is the published price of
/// the depth this adapter requests, marked quoted. Recording it as observed
/// would be a fabrication, and recording nothing would lose the fact that this
/// search was billable.
fn quoted_charge() -> AcquisitionCharge {
    // `fast` depth is documented at two credits per search.
    quoted_native(
        "credits",
        Number::from(2u64),
        "Nimble reports no cost, credit or usage field on the search response; quoted from the \
         published per-search price of the fast search depth (2 credits).",
    )
}
