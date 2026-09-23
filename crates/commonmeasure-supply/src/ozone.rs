//! Ozone Live: retrieval over a licensed publisher corpus, returning passages
//! with publisher identity, plus full text for a named URL.
//!
//! The wire format is built to Ozone's machine-readable specification
//! (`https://api.ozone.live/openapi.yaml`) and its API reference. The tests
//! beside this exercise the parser over the documented shapes through the
//! real transport and a loopback origin. The key is sent as a bearer token in
//! the documented `Authorization` header.
//!
//! Ozone is a retrieval API and nothing else: it never calls a model, never
//! generates an answer and rejects `include_answer`-style parameters with
//! `400`. Nothing this adapter sends asks for one.
//!
//! Wire facts that shape this adapter:
//!
//! - `top_k` is the documented result count (1–100, default 3), so the job's
//!   limit reaches the provider and is clamped to the published ceiling,
//!   which `maximum_search_results` declares so the runtime records any
//!   shortfall.
//! - `group_by` is sent as `article`: one row per document with the
//!   best-scoring passage as the row and the rest nested under
//!   `additional_chunks`. That keeps a result a page, as every other provider
//!   counts one, so a three-result job compares three documents and not three
//!   passages of one. The nested passages stay in native metadata; the
//!   envelope text is the best passage alone.
//! - `filters.domains` takes the job's whole allowed-host set in one request,
//!   sent only when the job names hosts. Ozone's `domain` field is a
//!   registrable domain (`bbc.com`, `coventrytelegraph.net`) while a job names
//!   full hosts, and whether Ozone matches `www.bbc.com` against `bbc.com` is
//!   unverified; admission enforces the host policy over the results either
//!   way.
//! - The envelope host is taken from `url`, never from `domain`: `domain` has
//!   been seen to name a publisher group's domain for a page on one of its
//!   other titles, and a host policy applied to it would rule on the wrong
//!   host. `domain` stays in native metadata.
//! - `licensed` is a boolean on every result and is fixed `true` on the
//!   search response. It says the passage was served from Ozone's licensed
//!   corpus; it does not name a licence or state the operator's permitted use,
//!   and no machine-readable agreement reference appears anywhere in the
//!   response. The envelope therefore stays `LicenceState::Unknown`, the
//!   boolean and the publisher identity (`publisher_id`, `publisher_name`)
//!   stay in native metadata, and this adapter declares no `licensed`
//!   capability. Promoting a boolean into a rights claim would be the TollBit
//!   mistake with a different field name.
//! - `published_date` is the documented per-result date and maps to the
//!   declared date with provenance naming the field. Ozone's own reference
//!   example dates a 2019 general-election article `2026-04-29`, so whether
//!   the field is a publication date or an ingestion date is an open question.
//! - The response carries `usage` (embedding tokens, cache hit, provider-side
//!   latency) but no cost field, and no price is published, so the charge is
//!   unknown, never zero. There is no request identifier, so
//!   `provider_request_id` stays `None`.
//! - `fetch` is `POST /v1/contents` with one URL. A corpus article comes from
//!   licensed storage (`source: gcs_markdown`, `licensed: true`); any other URL
//!   falls back to a live fetch by Ozone's own fetcher, marked
//!   `source: live_fetch`, `licensed: false`. Both are offered as supply with
//!   the outcome in native metadata, because which URL a job fetches is the
//!   job's decision; a live-fetched page is an ordinary open-web fetch under a
//!   third party's fetcher, exactly as it is for the other fetch providers,
//!   and it is licence-unknown like them. A per-item failure (`ok: false`)
//!   yields no envelope and stays sealed in the raw response.
//! - Every error is `{"error":{"code","message"}}` with a stable code. The
//!   shared transport already surfaces a non-2xx status with the body's own
//!   words, which is where the code lands.
//!
//! `/v1/publishers/domains`, the flat domain-to-publisher ownership map, is
//! not wired. It would make a cheap "does a licensed supplier hold this
//! host?" check for a router, which is a runtime decision and not an
//! acquisition; when the runtime has a consumer for it, it belongs beside the
//! declarations read before a crossing, not on this adapter.

use commonmeasure_http::Request;
use commonmeasure_types::{AcquisitionCharge, ContextEnvelope, DeclaredDate, ProviderCapability};
use serde_json::{Value, json};

use crate::{
    Acquisition, ResultFields, SupplyAdapter, SupplyError, envelopes_from_results, execute,
    parse_json, results_array,
};

pub(crate) const DEFAULT_BASE_URL: &str = "https://api.ozone.live";
/// Search discovers licensed passages; fetch retrieves a named URL through
/// `/v1/contents`. No `licensed`: see the module note on the boolean.
pub(crate) const CAPABILITIES: &[ProviderCapability] =
    &[ProviderCapability::Search, ProviderCapability::Fetch];

/// Ozone's documented `top_k` ceiling (`SearchRequest.top_k`, maximum 100).
const MAX_TOP_K: u32 = 100;

pub struct OzoneAdapter {
    base_url: String,
    api_key: String,
}

impl OzoneAdapter {
    /// `base_url` is a parameter so a test can point the adapter at a loopback
    /// origin serving a documented-shape response. The credential is held here
    /// and only ever written into a request header.
    pub fn new(base_url: &str, api_key: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key: api_key.to_owned(),
        }
    }

    fn post(&self, body: &Value) -> Result<Request, SupplyError> {
        let mut request = Request::post(
            "/",
            serde_json::to_vec(body).map_err(|error| SupplyError::Malformed {
                detail: format!("could not serialise the request: {error}"),
            })?,
            "application/json",
        );
        request
            .headers
            .set("Authorization", &format!("Bearer {}", self.api_key));
        request.headers.set("Accept", "application/json");
        Ok(request)
    }
}

impl SupplyAdapter for OzoneAdapter {
    fn provider(&self) -> &str {
        "ozone"
    }

    fn capabilities(&self) -> &'static [ProviderCapability] {
        CAPABILITIES
    }

    fn maximum_search_results(&self) -> Option<u32> {
        Some(MAX_TOP_K)
    }

    fn search(
        &self,
        query: &str,
        limit: u32,
        include_hosts: &[&str],
    ) -> Result<Acquisition, SupplyError> {
        let endpoint = format!("{}/v1/search", self.base_url);
        // The runtime records the shortfall against what the job asked for,
        // reading the cap from `maximum_search_results`.
        let mut body = json!({
            "query": query,
            "top_k": limit.clamp(1, MAX_TOP_K),
            "group_by": "article",
            "include_text": true,
        });
        // `filters.domains` scopes the search to the operator's allowed source
        // hosts. Absent when the job named none, so an unscoped search sends
        // the body it always did.
        if !include_hosts.is_empty() {
            body["filters"] = json!({
                "domains": {"values": include_hosts, "mode": "include"},
            });
        }
        let request = self.post(&body)?;

        let (response, latency_ms) = execute(&endpoint, request)?;
        let parsed = parse_json(&response.body)?;
        let envelopes = search_envelopes(&parsed, &endpoint)?;

        Ok(Acquisition {
            provider: self.provider().to_owned(),
            capability: ProviderCapability::Search,
            endpoint,
            http_status: Some(response.status),
            latency_ms,
            // The documented response has no request identifier (`query`,
            // `provider`, `group_by`, `results`, `usage`, `date_filter`), so
            // there is nothing to join a provider-side receipt to.
            provider_request_id: None,
            charge: charge_from(&parsed),
            envelopes,
            raw_response: response.body,
            invocation: None,
        })
    }

    fn fetch(&self, url: &str) -> Result<Acquisition, SupplyError> {
        let endpoint = format!("{}/v1/contents", self.base_url);
        let request = self.post(&json!({"urls": [url]}))?;

        let (response, latency_ms) = execute(&endpoint, request)?;
        let parsed = parse_json(&response.body)?;
        let envelopes = content_envelopes(&parsed, &endpoint)?;

        Ok(Acquisition {
            provider: self.provider().to_owned(),
            capability: ProviderCapability::Fetch,
            endpoint,
            http_status: Some(response.status),
            latency_ms,
            provider_request_id: None,
            charge: charge_from(&parsed),
            envelopes,
            raw_response: response.body,
            invocation: None,
        })
    }
}

/// One envelope per search row. With `group_by: article` a row is one
/// document carrying its best-scoring passage as `text`.
fn search_envelopes(body: &Value, endpoint: &str) -> Result<Vec<ContextEnvelope>, SupplyError> {
    let results = results_array(body, "/results", endpoint)?;
    Ok(envelopes_from_results(results, |item| {
        Some(ResultFields {
            url: item.get("url")?.as_str()?.to_owned(),
            // Ozone documents `title` as falling back to the section heading
            // and then a de-slugified URL segment when the corpus title is
            // empty, never a fabricated headline. It is mapped as the provider
            // returns it; an empty string is no title.
            title: item
                .get("title")
                .and_then(Value::as_str)
                .filter(|title| !title.is_empty())
                .map(str::to_owned),
            // `text` is the passage body, omitted when `include_text` is
            // false (this adapter always asks for it). `summary`,
            // `section_heading` and `additional_chunks` are different things
            // and stay in native metadata rather than being folded into the
            // retrieved text.
            text: item
                .get("text")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_owned),
            declared_date: declared_date(item, "search result"),
            promoted: &["url", "title", "text", "published_date"],
        })
    }))
}

/// One envelope per item `/v1/contents` served. A failed item (`ok: false`)
/// carries its own error and no text; it yields no envelope and stays in the
/// sealed response. A served item with an empty body is a textless
/// candidate, never invented content.
fn content_envelopes(body: &Value, endpoint: &str) -> Result<Vec<ContextEnvelope>, SupplyError> {
    let results = results_array(body, "/results", endpoint)?;
    Ok(envelopes_from_results(results, |item| {
        if item.get("ok") != Some(&Value::Bool(true)) {
            return None;
        }
        Some(ResultFields {
            url: item.get("url")?.as_str()?.to_owned(),
            // `ContentItem` documents no title field.
            title: None,
            text: item
                .get("text")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_owned),
            // `ContentItem` documents no date field. `source` and `licensed`
            // stay in native metadata: they say where Ozone got the bytes,
            // which a reviewer needs, and they are not a licence reference.
            declared_date: None,
            promoted: &["url", "text"],
        })
    }))
}

/// The documented per-result `published_date`, verbatim, with provenance
/// naming the field. A result without one stays undated rather than dated by
/// a neighbour.
fn declared_date(item: &Value, on: &str) -> Option<DeclaredDate> {
    item.get("published_date")
        .and_then(Value::as_str)
        .filter(|date| !date.is_empty())
        .map(|date| DeclaredDate {
            date: date.to_owned(),
            provenance: format!("supplier-declared: published_date on the Ozone Live {on}"),
        })
}

/// Ozone reports no cost field on any response.
///
/// The search response's `usage` block carries `embedding_tokens`,
/// `embedding_cached` and a provider-side `latency_ms`: consumption figures,
/// not a charge, and no price per token, per call or per passage is published
/// in the specification or the reference. So there is neither an observed
/// charge to record nor a published price to quote, and the charge is left
/// unknown rather than recorded as zero: an absent cost is not a free call.
/// The usage block stays in the sealed response.
fn charge_from(_body: &Value) -> AcquisitionCharge {
    AcquisitionCharge::default()
}
