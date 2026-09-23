//! Redpine Connect: licensed media mentions behind a native quote-then-buy
//! gateway.
//!
//! The surface is MCP JSON-RPC over Streamable HTTP: `initialize` issues a
//! session id in an `mcp-session-id`
//! response header, and the gateway's meta-tools front the actual
//! integrations. This adapter speaks exactly the recorded path — `initialize`,
//! `get_balance`, `inspect-tool`, `preview`, `confirm` — against the one
//! target tool the recon exercised, `media--sample`. `preview` executes the
//! tool and returns a teaser plus a billing note and a `preview_id`; `confirm`
//! unlocks the full result and returns the receipt (`cost_charged`,
//! `balance_remaining`). That is a quote → decision → settlement loop at the
//! API boundary, which is why this is the first adapter to declare `quote`.
//!
//! What is deliberately absent: `licensed`, because permitted use is not
//! machine-readable anywhere in the responses, and declaring the
//! capability would let a plan believe a rights claim could be obtained;
//! `fetch`, because `media--get_mention` exists in the catalogue but has
//! never been exercised; `corroborate`, because the receipt rides the confirm
//! response in-band and no provider-side receipt lookup exists to join to a
//! crossing identifier. The REST surface and the preview-bypassing
//! `call-tool` path are likewise unexercised and not implemented.

use commonmeasure_types::canonical::sha256_digest;
use std::str::FromStr;

use commonmeasure_http::Request;
use commonmeasure_types::{
    AcquisitionCharge, ChargeBasis, ContextEnvelope, DeclaredDate, LicenceState, NativeCharge,
    ProviderCapability,
};
use serde_json::{Value, json};

use crate::{
    ADAPTER_VERSION, Acquisition, SupplyAdapter, SupplyError, SupplyQuote, error_detail, execute,
    host_of, native_metadata, parse_json, results_array,
};

pub(crate) const DEFAULT_BASE_URL: &str = "https://api.redpine.ai/mcp";

/// `search` because a query discovers mentions and delivers their text;
/// `quote` because `preview` genuinely returns price and terms before
/// anything is bought. Declaring both says how the pair compose: search here
/// is implemented *behind the quote gate* (`quote` → purchase decision →
/// `confirm`), and the direct [`SupplyAdapter::search`] method deliberately
/// keeps its refusing default so no caller can buy without a decision.
/// Surfaces without a purchase decision must not dispatch to this provider.
pub(crate) const CAPABILITIES: &[ProviderCapability] =
    &[ProviderCapability::Search, ProviderCapability::Quote];

/// The one target tool the recorded evidence covers. It returns individual
/// mention records — with text — for a monitored keyword.
const TARGET_TOOL: &str = "media--sample";

/// `media--sample`'s page size cap (inspect-tool: "default 10, max 50").
const MAX_SAMPLE_LIMIT: u32 = 50;

/// The protocol version the recon spoke and the server echoed.
const PROTOCOL_VERSION: &str = "2025-03-26";

pub struct RedpineAdapter {
    base_url: String,
    api_key: String,
    user_agent: String,
}

impl RedpineAdapter {
    /// `base_url` is the full MCP endpoint, so a test can point the adapter
    /// at a loopback origin serving recorded Redpine responses. The credential is
    /// held here and only ever written into a request header.
    pub fn new(base_url: &str, api_key: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key: api_key.to_owned(),
            // The adapter and its version, not the `CommonMeasureBot`
            // product token: that name is the network identity a publisher
            // verifies by signature, and this call is to a supplier's API
            // under the operator's own credential.
            user_agent: ADAPTER_VERSION.to_owned(),
        }
    }

    /// One JSON-RPC exchange. A JSON-RPC `error` object and a tool result
    /// with `isError: true` are both the provider answering and refusing, so
    /// both surface as [`SupplyError::Status`] carrying the provider's own
    /// words — never as an empty result.
    fn rpc(
        &self,
        session: Option<&str>,
        id: u64,
        method: &str,
        params: Value,
    ) -> Result<(Value, Vec<u8>, u16, u64), SupplyError> {
        let mut body = json!({"jsonrpc": "2.0", "id": id, "method": method});
        if !params.is_null() {
            body["params"] = params;
        }
        let encoded = serde_json::to_vec(&body).map_err(|error| SupplyError::Malformed {
            detail: format!("could not serialise the request: {error}"),
        })?;
        let mut request = Request::post("/", encoded, "application/json");
        request
            .headers
            .set("Authorization", &format!("Bearer {}", self.api_key));
        request.headers.set("User-Agent", &self.user_agent);
        request.headers.set("Accept", "application/json");
        if let Some(session) = session {
            request.headers.set("Mcp-Session-Id", session);
        }
        let (response, latency_ms) = execute(&self.base_url, request)?;
        let parsed = parse_json(&response.body)?;
        if let Some(error) = parsed.get("error") {
            return Err(SupplyError::Status {
                status: response.status,
                endpoint: self.base_url.clone(),
                detail: format!("JSON-RPC error on {method}: {error}"),
            });
        }
        if parsed["result"]["isError"] == Value::Bool(true) {
            let words = parsed["result"]["content"][0]["text"]
                .as_str()
                .unwrap_or("the tool result says isError with no text");
            return Err(SupplyError::Status {
                status: response.status,
                endpoint: self.base_url.clone(),
                detail: format!(
                    "{method} answered an error result: {}",
                    error_detail(words.as_bytes())
                ),
            });
        }
        let status = response.status;
        Ok((parsed, response.body, status, latency_ms))
    }

    /// `initialize`, returning the session id the server issued in its
    /// `mcp-session-id` response header. A missing header is a session this
    /// adapter cannot continue, not a quieter mode.
    fn initialize(&self) -> Result<(String, u64), SupplyError> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "CommonMeasure", "version": ADAPTER_VERSION},
        }});
        let encoded = serde_json::to_vec(&body).map_err(|error| SupplyError::Malformed {
            detail: format!("could not serialise the request: {error}"),
        })?;
        let mut request = Request::post("/", encoded, "application/json");
        request
            .headers
            .set("Authorization", &format!("Bearer {}", self.api_key));
        request.headers.set("User-Agent", &self.user_agent);
        request.headers.set("Accept", "application/json");
        let (response, latency_ms) = execute(&self.base_url, request)?;
        // Surface a JSON-RPC-level refusal before complaining about the
        // missing header it explains.
        let parsed = parse_json(&response.body)?;
        if let Some(error) = parsed.get("error") {
            return Err(SupplyError::Status {
                status: response.status,
                endpoint: self.base_url.clone(),
                detail: format!("JSON-RPC error on initialize: {error}"),
            });
        }
        let session = response
            .headers
            .get("mcp-session-id")
            .map(str::to_owned)
            .ok_or_else(|| SupplyError::Malformed {
                detail: "initialize answered without an mcp-session-id response header, and \
                         every subsequent call requires one"
                    .to_owned(),
            })?;
        Ok((session, latency_ms))
    }
}

impl SupplyAdapter for RedpineAdapter {
    fn provider(&self) -> &str {
        "redpine"
    }

    fn capabilities(&self) -> &'static [ProviderCapability] {
        CAPABILITIES
    }

    /// Price a `media--sample` acquisition without buying it: `initialize`,
    /// `get_balance` (free; the balance before any purchase), `inspect-tool`
    /// (the machine-readable pricing annotation), `preview` (the billing
    /// note, trial state and `preview_id`). No leg consumes a trial query or
    /// charges the balance — only `confirm` does.
    ///
    /// `query` is used verbatim as the `filter.keyword` of `media--sample`,
    /// the one argument shape the recorded evidence covers. The keyword
    /// filter matches monitored keywords, not free text: a sentence-shaped
    /// query is unverified behaviour.
    fn quote(&self, query: &str, limit: u32) -> Result<SupplyQuote, SupplyError> {
        let (session, initialize_ms) = self.initialize()?;

        let (balance, _, _, balance_ms) = self.rpc(
            Some(&session),
            2,
            "tools/call",
            json!({"name": "get_balance", "arguments": {}}),
        )?;
        let balance_before = balance["result"]["structuredContent"]["balance"]
            .as_str()
            .map(str::to_owned);

        let (inspect, raw_inspect, _, inspect_ms) = self.rpc(
            Some(&session),
            3,
            "tools/call",
            json!({"name": "inspect-tool", "arguments": {"tool_name": TARGET_TOOL}}),
        )?;
        let charge = quoted_charge(&inspect);

        let (preview, raw_preview, _, preview_ms) = self.rpc(
            Some(&session),
            4,
            "tools/call",
            json!({"name": "preview", "arguments": {
                "tool_name": TARGET_TOOL,
                "arguments": {
                    "filter": {"keyword": query},
                    "sort": "recent",
                    "limit": limit.clamp(1, MAX_SAMPLE_LIMIT),
                },
            }}),
        )?;
        let quoted = &preview["result"]["structuredContent"];
        let preview_id = quoted["preview_id"]
            .as_str()
            .ok_or_else(|| SupplyError::Malformed {
                detail: "the preview carries no structuredContent.preview_id, so there is \
                         nothing a confirm could unlock"
                    .to_owned(),
            })?
            .to_owned();
        let workflow = quoted["workflow"]
            .as_str()
            .ok_or_else(|| SupplyError::Malformed {
                detail: "the preview carries no structuredContent.workflow, so whether a \
                         confirm is required cannot be read"
                    .to_owned(),
            })?
            .to_owned();

        Ok(SupplyQuote {
            provider: self.provider().to_owned(),
            endpoint: self.base_url.clone(),
            preview_id,
            workflow,
            billing_status: quoted["billing_status"].as_str().map(str::to_owned),
            billing_note: quoted["billing_note"].as_str().map(str::to_owned),
            trial_remaining: quoted["trial_remaining"].as_u64(),
            trial_total: quoted["trial_total"].as_u64(),
            balance_before,
            charge,
            latency_ms: initialize_ms + balance_ms + inspect_ms + preview_ms,
            raw_inspect,
            raw_preview,
            session,
        })
    }

    /// Buy what the quote priced. This is the only leg that consumes a trial
    /// query or charges the balance; the receipt's `cost_charged` and
    /// `balance_remaining` are recorded as the observed charge, and a
    /// trial-covered zero is recorded as trial coverage, never as free.
    fn confirm(&self, quote: &SupplyQuote) -> Result<Acquisition, SupplyError> {
        let (confirmed, raw_response, http_status, latency_ms) = self.rpc(
            Some(&quote.session),
            5,
            "tools/call",
            json!({"name": "confirm", "arguments": {"preview_id": quote.preview_id}}),
        )?;
        let receipt = &confirmed["result"]["structuredContent"];
        let cost_charged =
            receipt["cost_charged"]
                .as_str()
                .ok_or_else(|| SupplyError::Malformed {
                    detail: "the confirm receipt carries no structuredContent.cost_charged; \
                             without it the observed charge cannot be recorded"
                        .to_owned(),
                })?;
        let balance_remaining = receipt["balance_remaining"].as_str();
        // The full result arrives as a JSON-encoded string in
        // `structuredContent.content`. Unreadable content is an error, never
        // an empty acquisition that reads like "nothing matched".
        let content = receipt["content"]
            .as_str()
            .ok_or_else(|| SupplyError::Malformed {
                detail: "the confirm receipt carries no structuredContent.content string"
                    .to_owned(),
            })?;
        let payload: Value =
            serde_json::from_str(content).map_err(|error| SupplyError::Malformed {
                detail: format!("the confirm receipt's content is not valid JSON: {error}"),
            })?;

        let envelopes = envelopes_from(&payload, &self.base_url)?;
        Ok(Acquisition {
            provider: self.provider().to_owned(),
            capability: ProviderCapability::Search,
            endpoint: self.base_url.clone(),
            http_status: Some(http_status),
            latency_ms,
            // No request identifier appears anywhere in the responses; the
            // `preview_id` is the join between quote and receipt and is
            // carried in the plan's quote record.
            provider_request_id: None,
            charge: observed_charge(quote, cost_charged, balance_remaining),
            envelopes,
            raw_response,
            invocation: None,
        })
    }
}

/// The quoted price from `inspect-tool`'s pricing annotation.
///
/// The annotation rides inside the tool description text as
/// `- pricing: {'cost_credits': 0.03, 'cost_type': 'exact', ...}`
/// (a recorded inspect sample) — prose framing, so
/// the extraction is narrow and an unparseable annotation leaves the charge
/// absent (unknown, never zero) rather than guessed at. The note names the
/// twenty-fold disagreement with the payload's own `_meta.cost_usd`, which is
/// nothing in the responses says which figure a funded account
/// is charged.
fn quoted_charge(inspect: &Value) -> AcquisitionCharge {
    let Some(text) = inspect["result"]["content"][0]["text"].as_str() else {
        return AcquisitionCharge::default();
    };
    let Some(amount) = extract_number_after(text, "'cost_credits':") else {
        return AcquisitionCharge::default();
    };
    AcquisitionCharge {
        money: None,
        native: Some(NativeCharge {
            unit: "credits".to_owned(),
            amount,
            basis: ChargeBasis::Quoted,
            note: Some(
                "inspect-tool's pricing annotation for media--sample. The confirmed payload's \
                 own _meta.cost_usd has been seen to disagree with it twenty-fold, and nothing \
                 in the responses says which figure a funded account is charged."
                    .to_owned(),
            ),
        }),
    }
}

/// The number that follows `marker` in annotation text, parsed from its own
/// decimal digits so an exact price stays exact.
fn extract_number_after(text: &str, marker: &str) -> Option<serde_json::Number> {
    let after = &text[text.find(marker)? + marker.len()..];
    let digits: String = after
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    serde_json::Number::from_str(&digits).ok()
}

/// The observed charge from the confirm receipt.
///
/// A trial-covered purchase moved a meter, not a balance: the receipt's
/// `cost_charged` was zero while the trial meter observably went 5 → 4
/// (the recorded balance responses), so the honest unit is the
/// trial query and the note carries the receipt verbatim. Recording money at
/// zero here would be exactly the "free" that `docs/FAIL-POLICY.md` §7
/// forbids. Outside the trial the receipt's bare decimal is recorded in
/// credits — the unit Redpine's balance and pricing annotations use — with a
/// note saying the receipt itself does not name one (no
/// non-trial receipt has ever been observed).
fn observed_charge(
    quote: &SupplyQuote,
    cost_charged: &str,
    balance_remaining: Option<&str>,
) -> AcquisitionCharge {
    let receipt = format!(
        "the confirm receipt reports cost_charged {cost_charged} and balance_remaining {}",
        balance_remaining.unwrap_or("(not reported)")
    );
    if quote.billing_status.as_deref() == Some("trial") {
        return AcquisitionCharge {
            money: None,
            native: Some(NativeCharge {
                unit: "trial_queries".to_owned(),
                amount: 1.into(),
                basis: ChargeBasis::Observed,
                note: Some(format!(
                    "Covered by the free trial — a meter, not a price; {receipt}.{}",
                    quote
                        .billing_note
                        .as_deref()
                        .map(|note| format!(" Billing note: {note}"))
                        .unwrap_or_default()
                )),
            }),
        };
    }
    let amount = serde_json::Number::from_str(cost_charged.trim()).ok();
    AcquisitionCharge {
        money: None,
        native: amount.map(|amount| NativeCharge {
            unit: "credits".to_owned(),
            amount,
            basis: ChargeBasis::Observed,
            note: Some(format!(
                "The receipt does not name its unit; credits is the unit Redpine's balance and \
                 pricing annotations use; no response states it. \
                 {receipt}."
            )),
        }),
    }
}

/// Envelopes from the unlocked payload's `mentions`.
fn envelopes_from(payload: &Value, endpoint: &str) -> Result<Vec<ContextEnvelope>, SupplyError> {
    let envelopes = results_array(payload, "/mentions", endpoint)?
        .iter()
        .enumerate()
        .filter_map(|(index, mention)| {
            let url = mention.get("url")?.as_str()?.to_owned();
            let text = mention
                .get("text")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_owned);
            Some(ContextEnvelope {
                host: host_of(&url)?,
                content_hash: text.as_deref().map(|text| sha256_digest(text.as_bytes())),
                title: mention
                    .get("episode_title")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                text,
                // Permitted use is not machine-readable anywhere in the
                // responses. `_meta.data_source` names a data partnership,
                // which is provenance, not a licence; it stays in the payload
                // and is never promoted into a rights claim.
                licence: LicenceState::Unknown,
                declared_date: mention
                    .get("published_at")
                    .and_then(Value::as_str)
                    .map(|date| DeclaredDate {
                        date: date.to_owned(),
                        provenance: "supplier-declared: published_at on the Redpine mention"
                            .to_owned(),
                    }),
                native_metadata: native_metadata(
                    mention,
                    &["url", "text", "episode_title", "published_at"],
                ),
                retrieval_rank: index as u32 + 1,
                source_url: url,
            })
        })
        .collect();
    Ok(envelopes)
}
