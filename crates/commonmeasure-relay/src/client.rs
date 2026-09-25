//! The delivery client: one batch document, one POST, one status.
//!
//! `POST {receiver}/events` with the `event_batch` document as the body and
//! the API key in `X-API-Key`. The Content Telemetry standard defines no
//! response body, so any 2xx is acceptance, as the hub's onward delivery
//! counts it; requiring one receiver's body would make that receiver the only
//! conforming one. A body is optional. When it is JSON with an unsigned
//! `events_created`, that count of newly recorded events is kept (0 on a
//! redelivery, which is the idempotency working); otherwise the count is
//! unknown, never zero (`docs/FAIL-POLICY.md` §7). Anything other than a 2xx
//! is a delivery failure, and a failure keeps the spool.

use anyhow::{Result, bail};
use serde_json::Value;

/// What the receiver said about one accepted batch.
pub struct Acceptance {
    /// Events newly recorded by the receiver, when its answer states the
    /// count; `None` when it answered without one (a 204, an empty or
    /// non-JSON body, or JSON with no unsigned `events_created`).
    pub events_created: Option<u64>,
}

/// The product token every delivery carries. The wire document is the
/// standard's alone, so the delivering software names itself in transport
/// metadata instead; the receiver records it per batch, which is what makes
/// "which edges run what?" answerable from the fleet view.
pub const USER_AGENT: &str = concat!("commonmeasure-relay/", env!("CARGO_PKG_VERSION"));

pub fn deliver(receiver: &str, api_key: Option<&str>, document: &Value) -> Result<Acceptance> {
    let body = serde_json::to_vec(document)?;
    let mut request = commonmeasure_http::Request::post("/events", body, "application/json");
    request.headers.set("User-Agent", USER_AGENT);
    if let Some(key) = api_key {
        request.headers.set("X-API-Key", key);
    }
    let url = format!("{}/events", receiver.trim_end_matches('/'));
    let response = commonmeasure_http::send(&url, request)?;
    let answer = serde_json::from_slice::<Value>(&response.body).ok();
    if !(200..300).contains(&response.status) {
        let detail = answer
            .as_ref()
            .and_then(|parsed| parsed["detail"].as_str().map(str::to_owned))
            .unwrap_or_else(|| summarise(&response.body));
        bail!("receiver answered {}: {detail}", response.status);
    }
    Ok(Acceptance {
        events_created: answer.and_then(|answer| answer["events_created"].as_u64()),
    })
}

/// The body as it can be quoted in an error: one line, bounded, so an HTML
/// error page names itself in the receipts without filling them.
fn summarise(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return "an empty body".to_owned();
    }
    let head: String = collapsed.chars().take(160).collect();
    if head.chars().count() < collapsed.chars().count() {
        format!("{head}…")
    } else {
        head
    }
}
