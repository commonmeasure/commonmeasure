//! Exa: open-web neural search with page contents.
//!
//! `/contents` is the `fetch` capability. Exa is the only provider in the set
//! that reports its charge in currency.

use commonmeasure_http::Request;
use commonmeasure_types::{
    AcquisitionCharge, ChargeBasis, ContextEnvelope, DeclaredDate, Money, NativeCharge,
    ProviderCapability,
};
use serde_json::{Value, json};

use crate::{
    Acquisition, ResultFields, SupplyAdapter, SupplyError, envelopes_from_results, execute,
    parse_json, results_array,
};

pub(crate) const DEFAULT_BASE_URL: &str = "https://api.exa.ai";
/// Search discovers candidates with page text; fetch retrieves a named URL's
/// text through `/contents`. `licensed` is absent deliberately on both: no Exa
/// response carries a licence, rights or author field, and `author` is present
/// and null.
pub(crate) const CAPABILITIES: &[ProviderCapability] =
    &[ProviderCapability::Search, ProviderCapability::Fetch];

/// Characters of page text requested per result.
///
/// Fixed rather than derived from the job's context budget: a provider asked
/// for more text than another has been given an advantage the comparison did
/// not intend. Admission trims to the operator's budget afterwards.
const MAX_TEXT_CHARACTERS: u32 = 3000;

pub struct ExaAdapter {
    base_url: String,
    api_key: String,
}

impl ExaAdapter {
    /// `base_url` is a parameter so a test can point the adapter at a loopback
    /// server replaying recorded Exa responses. The credential is held here and
    /// only ever written into a request header.
    pub fn new(base_url: &str, api_key: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key: api_key.to_owned(),
        }
    }
}

impl SupplyAdapter for ExaAdapter {
    fn provider(&self) -> &str {
        "exa"
    }

    fn capabilities(&self) -> &'static [ProviderCapability] {
        CAPABILITIES
    }

    /// Exa's published prices, spec-verified 1 August 2026 against
    /// <https://exa.ai/docs/reference/pricing> — USD 7 per 1k search requests
    /// and USD 1 per 1k content pages — and the observed live charges matched
    /// them exactly (`costDollars.total` 0.007 and 0.001). If Exa's pricing
    /// page moves, this moves with it; the allowance ledger reconciles the
    /// reservation against the observed charge either way, so a drifted
    /// figure is visible on every receipt.
    fn published_price(&self, capability: ProviderCapability) -> Option<Money> {
        match capability {
            ProviderCapability::Search => Some(Money::new("USD", 7_000)),
            ProviderCapability::Fetch => Some(Money::new("USD", 1_000)),
            _ => None,
        }
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
            "numResults": limit,
            "contents": {"text": {"maxCharacters": MAX_TEXT_CHARACTERS}},
        });
        // `includeDomains` restricts the neural search to the operator's
        // allowed source hosts. Absent when the job named none, so an unscoped
        // search sends exactly the body it always did; present, it asks Exa for
        // only the hosts admission would keep anyway.
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
        request.headers.set("x-api-key", &self.api_key);
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
                .get("requestId")
                .and_then(Value::as_str)
                .map(str::to_owned),
            charge: charge_from(&parsed),
            envelopes,
            raw_response: response.body,
            invocation: None,
        })
    }

    fn fetch(&self, url: &str) -> Result<Acquisition, SupplyError> {
        let endpoint = format!("{}/contents", self.base_url);
        // The same page-text request as search, aimed at one named URL: the
        // `/contents` result shape matches a search result, so the parser and
        // the cost rule are shared.
        let body = json!({
            "urls": [url],
            "text": {"maxCharacters": MAX_TEXT_CHARACTERS},
        });
        let mut request = Request::post(
            "/",
            serde_json::to_vec(&body).map_err(|error| SupplyError::Malformed {
                detail: format!("could not serialise the request: {error}"),
            })?,
            "application/json",
        );
        request.headers.set("x-api-key", &self.api_key);
        request.headers.set("Accept", "application/json");

        let (response, latency_ms) = execute(&endpoint, request)?;
        let parsed = parse_json(&response.body)?;
        let envelopes = envelopes_from(&parsed, &endpoint)?;

        Ok(Acquisition {
            provider: self.provider().to_owned(),
            capability: ProviderCapability::Fetch,
            endpoint,
            http_status: Some(response.status),
            latency_ms,
            provider_request_id: parsed
                .get("requestId")
                .and_then(Value::as_str)
                .map(str::to_owned),
            charge: charge_from(&parsed),
            envelopes,
            raw_response: response.body,
            invocation: None,
        })
    }
}

/// No Exa response carries licence metadata, so every envelope stays unknown.
fn envelopes_from(body: &Value, endpoint: &str) -> Result<Vec<ContextEnvelope>, SupplyError> {
    let results = results_array(body, "/results", endpoint)?;
    Ok(envelopes_from_results(results, |item| {
        Some(ResultFields {
            url: item.get("url")?.as_str()?.to_owned(),
            title: item.get("title").and_then(Value::as_str).map(str::to_owned),
            // An empty string stays an empty string: "returned nothing" and
            // "returned emptiness" are different facts about the provider
            // (`commonmeasure_types::ContextEnvelope::text`).
            text: item.get("text").and_then(Value::as_str).map(str::to_owned),
            // `publishedDate` is Exa's documented per-result date, in the same
            // optional metadata family as the `author` the recon captures show
            // arriving null. Neither recorded search result carried one
            // so on those bytes this maps nothing — the
            // mapping itself is spec-verified only, and the date, where one
            // arrives, is Exa's claim about the page, untested against when the
            // content was written.
            declared_date: item
                .get("publishedDate")
                .and_then(Value::as_str)
                .map(|date| DeclaredDate {
                    date: date.to_owned(),
                    provenance: "supplier-declared: publishedDate on the Exa search result"
                        .to_owned(),
                }),
            promoted: &["url", "title", "text", "publishedDate"],
        })
    }))
}

/// Read the charge from `costDollars.total`, never from the per-category
/// breakdown.
///
/// Observed in captures of 1 August 2026: a combined search-and-contents
/// call attributed the entire charge to `search.neural` and carried no
/// `contents` line at all, so the categories cannot be used to separate
/// acquisition costs. The total matched the published price exactly.
fn charge_from(body: &Value) -> AcquisitionCharge {
    let Some(total) = body
        .pointer("/costDollars/total")
        .and_then(Value::as_number)
    else {
        return AcquisitionCharge::default();
    };
    // Parsed from the number's own decimal text: a charge that is already
    // exact should not acquire a rounding error on its way into evidence.
    let decimal = total.to_string();
    let money = Money::from_decimal_str("USD", &decimal);
    // A total finer than a micro-dollar, or written in exponent form, has no
    // comparable amount — but it is still a disclosed price, and dropping it
    // would leave the record indistinguishable from a provider that disclosed
    // nothing. The observed figure stays, and says why it could not be
    // compared, so a cost cap that cannot be enforced is enforceably absent
    // rather than quietly unenforced.
    let note = money.is_none().then(|| {
        format!(
            "Exa reported {decimal} USD, which the ledger cannot hold at micro-dollar precision, \
             so no comparable amount was derived from it."
        )
    });
    AcquisitionCharge {
        money,
        native: Some(NativeCharge {
            unit: "USD".to_owned(),
            amount: total.clone(),
            basis: ChargeBasis::Observed,
            note,
        }),
    }
}
