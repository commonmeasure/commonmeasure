//! The delivery client: one batch document, one POST, one parsed answer.
//!
//! Designed from the receiver's published API reference (oa-server's
//! `docs/api-reference.md`), not from its handlers: `POST {receiver}/events`
//! with the `event_batch` document as the body and the API key in `X-API-Key`,
//! answered 200 or 201 with `{"status": "ok", "events_created": n}` where
//! `events_created` counts only newly inserted events, so a fully redelivered
//! batch is a success that created nothing.
//!
//! That answer is the whole of acceptance. A 2xx alone proves only that
//! something on the other end of the socket replied — a proxy, a captive
//! portal, a wrong path on the right host — and accepting it would burn the
//! batch's event ids against a receiver that never recorded them, leaving the
//! true receiver permanently unreachable for those facts. Anything that is not
//! the documented answer is therefore a delivery failure, and a failure keeps
//! the spool.

use anyhow::{Result, bail};
use serde_json::{Value, json};

/// What the receiver said about one accepted batch.
pub struct Acceptance {
    /// Events newly recorded by the receiver; a redelivery reports 0 here and
    /// that is the idempotency working, not a failure.
    pub events_created: u64,
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
    if !matches!(response.status, 200 | 201) {
        let detail = answer
            .as_ref()
            .and_then(|parsed| parsed["detail"].as_str().map(str::to_owned))
            .unwrap_or_else(|| summarise(&response.body));
        bail!("receiver answered {}: {detail}", response.status);
    }
    let Some(answer) = answer.filter(|answer| answer["status"] == json!("ok")) else {
        bail!(
            "receiver answered {} but not as a telemetry receiver: expected a JSON body with \
             status \"ok\", got {}",
            response.status,
            summarise(&response.body)
        );
    };
    let Some(events_created) = answer["events_created"].as_u64() else {
        bail!(
            "receiver answered {} with status \"ok\" but no events_created count, so nothing \
             says the batch was recorded: {}",
            response.status,
            summarise(&response.body)
        );
    };
    Ok(Acceptance { events_created })
}

/// The body as it can be quoted in an error: one line, bounded, so an HTML
/// error page or a captive portal's login form names itself in the receipts
/// without filling them.
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
