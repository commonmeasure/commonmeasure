//! TollBit: metered publisher content, search leg only.
//!
//! The verification state, its dated evidence and the per-operation basis
//! are `docs/knowledge-base/provider-verification.md`.
//!
//! Every priced operation failed: `/dev/v2/rates/{url}` answered `403` on all
//! three URLs tried, including two marked `readyToLicense: true`, and
//! content-token creation answered `500`. No TollBit price has been observed
//! and no licensed fetch has succeeded, so the rate, token and content legs
//! are not implemented here: they would be code with no evidence behind it and
//! no caller on the run path.

use commonmeasure_http::Request;
use commonmeasure_types::{
    AcquisitionCharge, ContextEnvelope, DeclaredDate, LicenceState, ProviderCapability,
};
use serde_json::Value;

use crate::{
    ADAPTER_VERSION, Acquisition, SupplyAdapter, SupplyError, execute, host_of, native_metadata,
    parse_json, results_array, urlencode,
};

pub(crate) const DEFAULT_BASE_URL: &str = "https://gateway.tollbit.com";
/// Search only. `licensed` is deliberately absent: TollBit returns publisher
/// identity, which is not a machine-readable licence reference, and declaring
/// the capability would let a plan believe a rights claim could be obtained.
pub(crate) const CAPABILITIES: &[ProviderCapability] = &[ProviderCapability::Search];

/// TollBit's search page size is capped at 20 (OpenAPI, `size`).
const MAX_SEARCH_SIZE: u32 = 20;

pub struct TollbitAdapter {
    base_url: String,
    api_key: String,
    user_agent: String,
}

impl TollbitAdapter {
    pub fn new(base_url: &str, api_key: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key: api_key.to_owned(),
            // TollBit's correlation runs through the User-Agent, so this
            // names the adapter and its version (UNVERIFIED U6). Not the
            // `CommonMeasureBot` product token: that name is the network
            // identity a publisher verifies by signature, this call is to a
            // supplier's API under the operator's own credential, and it
            // carries no signature (`DECISIONS.md` §Product).
            user_agent: ADAPTER_VERSION.to_owned(),
        }
    }
}

impl SupplyAdapter for TollbitAdapter {
    fn provider(&self) -> &str {
        "tollbit"
    }

    fn capabilities(&self) -> &'static [ProviderCapability] {
        CAPABILITIES
    }

    // `include_hosts` is ignored: TollBit's search takes no domain filter this
    // adapter wires, so the search is unscoped and admission enforces the
    // operator's host policy over the results, exactly as before.
    fn maximum_search_results(&self) -> Option<u32> {
        Some(MAX_SEARCH_SIZE)
    }

    fn search(
        &self,
        query: &str,
        limit: u32,
        _include_hosts: &[&str],
    ) -> Result<Acquisition, SupplyError> {
        // The runtime records the shortfall against what the job asked for,
        // reading the cap from `maximum_search_results`.
        let size = limit.clamp(1, MAX_SEARCH_SIZE);
        let endpoint = format!(
            "{}/dev/v2/search?q={}&size={size}",
            self.base_url,
            urlencode(query)
        );
        let mut request = Request::get("/");
        request.headers.set("TollbitKey", &self.api_key);
        request.headers.set("User-Agent", &self.user_agent);
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
            // No request identifier appears in the search response body or
            // headers, so there is nothing to join a provider-side receipt to.
            provider_request_id: None,
            // Search is not a priced operation and TollBit reports no charge on
            // it. Unknown, not free.
            charge: AcquisitionCharge::default(),
            envelopes,
            raw_response: response.body,
            invocation: None,
        })
    }
}

fn envelopes_from(body: &Value, endpoint: &str) -> Result<Vec<ContextEnvelope>, SupplyError> {
    let envelopes = results_array(body, "/items", endpoint)?
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let url = item.get("url")?.as_str()?.to_owned();
            Some(ContextEnvelope {
                host: host_of(&url)?,
                title: item.get("title").and_then(Value::as_str).map(str::to_owned),
                // v2 search returns no excerpt at all. A candidate with no text
                // cannot ground an answer, so the envelope carries no text and
                // no content hash rather than a manufactured excerpt.
                text: None,
                content_hash: None,
                // `publisher` names who owns the page. It is not a licence:
                // `readyToLicense` did not survive contact with the rates
                // endpoint, and no machine-readable licence reference was ever
                // obtained.
                licence: LicenceState::Unknown,
                // `publishedDate` arrived on every recorded search item
                // (`demo/recon/tollbit/`) — TollBit is the one provider in
                // the set whose recorded bytes declare a date at all. It is
                // the supplier's claim about the page, trusted as declared
                // and never verified against the content.
                declared_date: item
                    .get("publishedDate")
                    .and_then(Value::as_str)
                    .map(|date| DeclaredDate {
                        date: date.to_owned(),
                        provenance: "supplier-declared: publishedDate on the TollBit search item"
                            .to_owned(),
                    }),
                native_metadata: native_metadata(item, &["url", "title", "publishedDate"]),
                retrieval_rank: index as u32 + 1,
                source_url: url,
            })
        })
        .collect();
    Ok(envelopes)
}
