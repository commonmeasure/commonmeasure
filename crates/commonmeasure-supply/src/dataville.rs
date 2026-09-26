//! Dataville: one record from one public source per search.
//!
//! Wire decisions follow the Dataville search, response-format, plans-and-credits,
//! errors and authentication references (`https://docs.dataville.com/llms.txt`).
//! Authenticated wiki and arxiv calls recorded on 25 September 2026 are exercised
//! through the production transport in `tests/dataville_spec.rs`.
//!
//! - `GET /{source}/{keywords}` returns one record. The cap is one regardless
//!   of the job's limit, so the runtime can record a coverage gap. Keywords are
//!   percent-encoded as one path segment; the key goes only in the bearer header.
//!   Neither `summary` nor the credential-bearing `token` query is sent.
//! - The first mapped host in job order selects the source. Only the two sources
//!   with recorded URL fields are wired: `en.wikipedia.org` to `wiki`, and
//!   `arxiv.org` to `arxiv`. With no hosts, wiki is the catalogue's general-reference
//!   source. With hosts but no mapping, the adapter refuses before spending credit.
//!   Admission still checks the returned URL against the job's host policy.
//! - Wiki's canonical URL is `metadata.url`; arxiv's is `metadata.abs_url`.
//!   The OpenAPI example shows `pdf_url`, but the response-format reference and
//!   recorded response carry the abstract URL (`http`, version suffix, null PDF).
//!   No URL means no envelope.
//! - `title` and non-empty `body` become title and text. `last_updated` is a
//!   supplier-declared update time; on wiki it dates the revision, not
//!   publication. On the arxiv capture it equals `metadata.published`, but the
//!   field is still recorded as an update time.
//!   Entities, language, source, metadata, attribution and any stale-copy notice
//!   remain in native metadata. A stored copy is not presented as a live result
//!   stripped of its notice. The wiki capture names `source: wikipedia` and
//!   `metadata.served_from: snapshot` with a `snapshot_date`, but has no `stale`
//!   object; these facts remain verbatim rather than inferred as a fresh lookup.
//! - `attribution.license.url` is Dataville's declaration about the upstream
//!   licence, not an independent rights check. It becomes `Declared`; an absent
//!   reference stays `Unknown`. No `licensed` capability is declared.
//! - `usage.request_cost` is an observed USD charge. The recorded wiki and arxiv
//!   charges (0.03 and 0.01) differ from published Free rates (0.025 and 0.005),
//!   so no rate table substitutes for them. Both replies show the same rounded
//!   remaining balance, so no per-request charge is derived from balances.
//!   An absent charge stays unknown; the other usage fields stay in the raw
//!   response.
//! - A rejected key can return HTTP 200 with `account_state: anonymous`. That is
//!   a failure naming `DATAVILLE_API_KEY`, never a successful unconfigured tier.
//!   `status: error` also fails on 200; non-2xx bodies use the shared transport's
//!   status error. Missing success or authentication fields are malformed replies.
//! - No request identifier is documented or observed, so none is invented.
//!   Wikipedia-only URL lookup cannot fulfil general `fetch`; search is the sole
//!   capability. Multi-source fan-out and Wikipedia URL lookup are not wired.

use commonmeasure_http::Request;
use commonmeasure_types::{
    AcquisitionCharge, ChargeBasis, DeclaredDate, LicenceState, Money, NativeCharge,
    ProviderCapability,
};
use serde_json::Value;

use crate::{
    Acquisition, ResultFields, SupplyAdapter, SupplyError, envelopes_from_results, error_detail,
    execute, parse_json, required_field, urlencode,
};

pub(crate) const DEFAULT_BASE_URL: &str = "https://api.dataville.com";
pub(crate) const CAPABILITIES: &[ProviderCapability] = &[ProviderCapability::Search];

/// Search Dataville's recorded Wikipedia and arXiv sources, one record per call.
pub struct DatavilleAdapter {
    base_url: String,
    api_key: String,
}

impl DatavilleAdapter {
    /// The origin is configurable so recorded responses can traverse the same
    /// transport and parser on loopback. The credential is sent only in a header.
    pub fn new(base_url: &str, api_key: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key: api_key.to_owned(),
        }
    }
}

fn source_for(include_hosts: &[&str]) -> Result<&'static str, SupplyError> {
    if include_hosts.is_empty() {
        return Ok("wiki");
    }
    for host in include_hosts {
        match *host {
            "en.wikipedia.org" => return Ok("wiki"),
            "arxiv.org" => return Ok("arxiv"),
            _ => {}
        }
    }
    // No pre-request refusal variant carries detail. CapabilityUnavailable would
    // wrongly say search is unimplemented; Transport records that no call completed.
    Err(SupplyError::Transport {
        detail: format!(
            "Dataville serves none of the requested hosts through this adapter: {}; no request sent",
            include_hosts.join(", ")
        ),
    })
}

impl SupplyAdapter for DatavilleAdapter {
    fn provider(&self) -> &str {
        "dataville"
    }

    fn capabilities(&self) -> &'static [ProviderCapability] {
        CAPABILITIES
    }

    fn maximum_search_results(&self) -> Option<u32> {
        Some(1)
    }

    fn search(
        &self,
        query: &str,
        _limit: u32,
        include_hosts: &[&str],
    ) -> Result<Acquisition, SupplyError> {
        let source = source_for(include_hosts)?;
        let endpoint = format!("{}/{source}/{}", self.base_url, urlencode(query));
        let mut request = Request::get("/");
        request
            .headers
            .set("Authorization", &format!("Bearer {}", self.api_key));
        request.headers.set("Accept", "application/json");
        let (response, latency_ms) = execute(&endpoint, request)?;
        let parsed = parse_json(&response.body)?;
        if parsed["status"] == "error" {
            return Err(SupplyError::Status {
                status: response.status,
                endpoint,
                detail: format!(
                    "{}: {}",
                    parsed
                        .pointer("/data/code")
                        .and_then(Value::as_str)
                        .unwrap_or("status: error"),
                    error_detail(
                        parsed
                            .pointer("/data/error")
                            .and_then(Value::as_str)
                            .unwrap_or("Dataville returned status: error without data.error")
                            .as_bytes()
                    ),
                ),
            });
        }
        if parsed["account_state"] == "anonymous" {
            return Err(SupplyError::Status {
                status: response.status,
                endpoint,
                detail: "Dataville rejected the key in DATAVILLE_API_KEY and answered on the anonymous tier".to_owned(),
            });
        }
        if parsed["status"] != "success" || parsed["account_state"] != "authenticated" {
            return Err(SupplyError::Malformed {
                detail: format!(
                    "{endpoint} did not declare status: success and account_state: authenticated"
                ),
            });
        }
        let data = required_field(&parsed, "/data", &endpoint)?;
        if !data.is_object() {
            return Err(SupplyError::Malformed {
                detail: format!("{endpoint} carries data as something other than an object"),
            });
        }
        let url_field = match source {
            "wiki" => "/metadata/url",
            "arxiv" => "/metadata/abs_url",
            _ => unreachable!("source_for returns only mapped sources"),
        };
        let envelopes = envelopes_from_results([data], |item| {
            Some(ResultFields {
                url: nonempty(item.pointer(url_field))?,
                title: nonempty(item.get("title")),
                text: nonempty(item.get("body")),
                declared_date: nonempty(item.get("last_updated")).map(|date| DeclaredDate {
                    date,
                    provenance: match source {
                        "wiki" => "supplier-declared: data.last_updated on Dataville wiki; the revision time, not the publication date".to_owned(),
                        _ => format!("supplier-declared: data.last_updated on Dataville {source}; an update time"),
                    },
                }),
                licence: nonempty(item.pointer("/attribution/license/url"))
                    .map(|reference| LicenceState::Declared { reference })
                    .unwrap_or(LicenceState::Unknown),
                promoted: &["title", "body", "last_updated"],
            })
        });
        Ok(Acquisition {
            provider: self.provider().to_owned(),
            capability: ProviderCapability::Search,
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

fn nonempty(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn charge_from(body: &Value) -> AcquisitionCharge {
    let Some(total) = body
        .pointer("/usage/request_cost")
        .and_then(Value::as_number)
    else {
        return AcquisitionCharge::default();
    };
    let decimal = total.to_string();
    let money = Money::from_decimal_str("USD", &decimal);
    let note = money.is_none().then(|| format!(
        "Dataville reported {decimal} USD, which the ledger cannot hold at micro-dollar precision, so no comparable amount was derived from it."
    ));
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
