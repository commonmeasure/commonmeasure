//! The Redpine adapter, over the real transport, against the bytes the
//! provider actually returned on 4 August 2026 (`recon/redpine/`).
//!
//! One loopback origin serves all five recorded legs — `initialize`,
//! `get_balance`, `inspect-tool`, `preview`, `confirm` — routing each request
//! by its own JSON-RPC body, exactly as the gateway does. Nothing in the path
//! is a stand-in: the client, the framing, the session discipline and the
//! parsers are the ones a live call uses. Only the origin is local: it stands
//! in for the network, never for the code under test.
//!
//! The captures' licensed transcript text is truncated (structure and
//! provenance survive; the licensed words do not), so no assertion here
//! touches mention text content beyond its presence and hash.

use commonmeasure_http::Response;
use commonmeasure_supply::{RedpineAdapter, SupplyAdapter, SupplyError};
use commonmeasure_types::{ChargeBasis, LicenceState};
use serde_json::Value;

mod common;

use common::{Origin, SeenRequest};

/// `initialize` for the handshake, else the gateway tool being called.
fn leg(request: &SeenRequest) -> String {
    let body = request.json_body();
    match body["method"].as_str() {
        Some("initialize") => "initialize".to_owned(),
        Some("tools/call") => body["params"]["name"]
            .as_str()
            .expect("a tools/call names its tool")
            .to_owned(),
        other => panic!("unexpected JSON-RPC method {other:?}"),
    }
}

/// Serve the five recorded legs, routing each request by its JSON-RPC body the
/// way the real gateway does. `initialize` answers with the capture's own
/// `mcp-session-id` response header (the committed redaction marker), so the
/// adapter's session discipline is exercised end to end: what the origin issues
/// is what every later request must present.
fn replaying_recon() -> Origin {
    Origin::serving(|request| {
        let body: Value =
            serde_json::from_slice(&request.body).expect("request body should be JSON");
        let capture = match body["method"].as_str() {
            Some("initialize") => "redpine-mcp-initialize.json",
            Some("tools/call") => match body["params"]["name"].as_str() {
                Some("get_balance") => "redpine-balance-before.json",
                Some("inspect-tool") => "redpine-inspect-sample-2.json",
                Some("preview") => "redpine-preview-sample.json",
                Some("confirm") => "redpine-confirm-sample.json",
                other => panic!("no recorded capture for tool {other:?}"),
            },
            other => panic!("no recorded capture for method {other:?}"),
        };
        recorded_response(capture)
    })
}

/// A committed Redpine capture, exactly as it was received: same status, same
/// body bytes (the capture's `body` is the wire body verbatim, as a string),
/// and the recorded `mcp-session-id` response header where the capture
/// carries one.
fn recorded_response(capture: &str) -> Response {
    let recorded = read_capture(capture);
    let status = recorded["status"]
        .as_u64()
        .expect("capture records a status") as u16;
    let body = recorded["body"]
        .as_str()
        .expect("a Redpine capture's body is the wire body as a string")
        .as_bytes()
        .to_vec();
    let mut response = Response::new(status, body);
    response.headers.set("Content-Type", "application/json");
    if let Some(session) = recorded["headers"]["mcp-session-id"].as_str() {
        response.headers.set("mcp-session-id", session);
    }
    response
}

/// A committed Redpine capture, parsed; `name` is the file beneath
/// `recon/redpine/`.
fn read_capture(name: &str) -> Value {
    common::read_capture(&format!("redpine/{name}"))
}

/// The session marker the initialize capture issues; every request after
/// `initialize` must present exactly what the origin issued.
fn issued_session() -> String {
    read_capture("redpine-mcp-initialize.json")["headers"]["mcp-session-id"]
        .as_str()
        .expect("the initialize capture records the session header")
        .to_owned()
}

#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn quote_reads_the_recorded_price_billing_note_and_balance_without_buying() {
    let origin = replaying_recon();
    let adapter = RedpineAdapter::new(&origin.url(), "test-key");

    let quote = adapter
        .quote("NVIDIA", 3)
        .expect("the recorded legs should parse");

    // The wire side: four legs in the recorded order, and never a confirm —
    // a quote spends nothing.
    let requests = origin.requests();
    let legs: Vec<String> = requests.iter().map(leg).collect();
    assert_eq!(
        legs,
        ["initialize", "get_balance", "inspect-tool", "preview"]
    );
    assert_eq!(
        requests[0].header("Authorization"),
        Some("Bearer test-key"),
        "the credential travels only in the request header"
    );
    assert!(
        requests[0].header("Mcp-Session-Id").is_none(),
        "initialize is what issues the session; it cannot present one"
    );
    let session = issued_session();
    for request in &requests[1..] {
        assert_eq!(
            request.header("Mcp-Session-Id"),
            Some(session.as_str()),
            "every leg after initialize presents the session the origin issued"
        );
    }
    // `inspect-tool`'s parameter is `tool_name`, not `name`; guessing `name`
    // earns a polite error from the real gateway (NOTES.md).
    let inspect = requests[2].json_body();
    assert_eq!(inspect["params"]["arguments"]["tool_name"], "media--sample");
    // The preview executes the target tool with the recorded argument shape.
    let preview = requests[3].json_body();
    let arguments = &preview["params"]["arguments"]["arguments"];
    assert_eq!(arguments["filter"]["keyword"], "NVIDIA");
    assert_eq!(arguments["sort"], "recent");
    assert_eq!(arguments["limit"], 3);

    // The quote itself: everything the plan evidence needs before a purchase
    // decision exists.
    assert_eq!(quote.provider, "redpine");
    assert_eq!(quote.preview_id, "pv_8d491df883034fec");
    assert_eq!(quote.workflow, "requires_confirm");
    assert_eq!(quote.billing_status.as_deref(), Some("trial"));
    assert!(
        quote
            .billing_note
            .as_deref()
            .expect("the recorded preview carries a billing note")
            .contains("Covered by free trial"),
        "the provider's own billing sentence survives verbatim"
    );
    assert_eq!(quote.trial_remaining, Some(5));
    assert_eq!(quote.trial_total, Some(5));
    assert_eq!(quote.balance_before.as_deref(), Some("0.00"));

    // The quoted price is inspect-tool's pricing annotation, in the
    // provider's own unit, marked quoted, with the pricing disagreement named.
    assert!(quote.charge.money.is_none(), "no currency was quoted");
    let native = quote
        .charge
        .native
        .as_ref()
        .expect("the annotation prices the tool");
    assert_eq!(native.unit, "credits");
    assert_eq!(native.amount.to_string(), "0.03");
    assert_eq!(native.basis, ChargeBasis::Quoted);
    assert!(
        native
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("cost_usd"),
        "the note names the twenty-fold disagreement"
    );

    // The raw bytes of both priced legs come back for sealing.
    assert!(!quote.raw_inspect.is_empty());
    assert!(!quote.raw_preview.is_empty());
}

#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn confirm_parses_the_licensed_mentions_and_records_a_trial_covered_charge() {
    let origin = replaying_recon();
    let adapter = RedpineAdapter::new(&origin.url(), "test-key");

    let quote = adapter.quote("NVIDIA", 3).expect("recorded quote");
    let acquisition = adapter.confirm(&quote).expect("recorded confirm");

    // The confirm leg presented the session and the quote's preview_id.
    let requests = origin.requests();
    let confirm = requests.last().expect("confirm was sent");
    assert_eq!(leg(confirm), "confirm");
    assert_eq!(
        confirm.header("Mcp-Session-Id"),
        Some(issued_session().as_str())
    );
    assert_eq!(
        confirm.json_body()["params"]["arguments"]["preview_id"],
        "pv_8d491df883034fec"
    );

    assert_eq!(acquisition.provider, "redpine");
    assert_eq!(acquisition.http_status, Some(200));
    assert_eq!(acquisition.envelopes.len(), 3);

    let first = &acquisition.envelopes[0];
    assert_eq!(first.host, "www.cnbc.com");
    assert_eq!(first.retrieval_rank, 1);
    assert_eq!(
        first.title.as_deref(),
        Some("Mad Money w/ Jim Cramer 8/3/26")
    );
    assert!(first.text.is_some(), "confirm unlocks mention text");
    assert!(
        first
            .content_hash
            .as_deref()
            .unwrap()
            .starts_with("sha256:"),
        "admitted text must be hashed at ingestion"
    );

    for envelope in &acquisition.envelopes {
        // R4: permitted use is not machine-readable anywhere in the
        // responses, and `_meta.data_source` names a partnership, not a
        // licence. Every acquisition carries explicit unknown.
        assert_eq!(envelope.licence, LicenceState::Unknown);
        // Every recorded mention declares `published_at`; the envelope
        // carries it verbatim with provenance naming who declared it.
        let date = envelope
            .declared_date
            .as_ref()
            .expect("every recorded Redpine mention declares published_at");
        assert!(
            date.provenance.contains("supplier-declared") && date.provenance.contains("Redpine"),
            "the provenance must name who declared the date: {}",
            date.provenance
        );
        // Promoted fields do not appear twice under two names; the
        // provider's own fields stay namespaced.
        assert!(envelope.native_metadata.get("published_at").is_none());
        assert!(envelope.native_metadata.get("channel_name").is_some());
    }
    assert_eq!(
        acquisition.envelopes[0]
            .declared_date
            .as_ref()
            .unwrap()
            .date,
        "2026-08-03T23:32:42Z"
    );

    // The trial covered the charge: a meter moved, no price was paid, and
    // recording money at zero would be the "free" the fail policy forbids.
    assert!(
        acquisition.charge.money.is_none(),
        "a trial-covered purchase observed no currency"
    );
    let native = acquisition
        .charge
        .native
        .as_ref()
        .expect("the trial query is the observed unit");
    assert_eq!(native.unit, "trial_queries");
    assert_eq!(native.amount.to_string(), "1");
    assert_eq!(native.basis, ChargeBasis::Observed);
    let note = native.note.as_deref().unwrap_or_default();
    assert!(
        note.contains("cost_charged 0.000000") && note.contains("meter, not a price"),
        "the receipt survives verbatim and trial coverage is named: {note}"
    );

    // The exact confirm bytes are sealed for the run to publish.
    let recorded = read_capture("redpine-confirm-sample.json");
    assert_eq!(
        acquisition.raw_response,
        recorded["body"].as_str().unwrap().as_bytes(),
        "the sealed response is the recorded wire body, byte for byte"
    );
}

/// An initialize that answers without a session header is a session this
/// adapter cannot continue. It must fail then and there, not send three more
/// legs that each earn a "missing session" refusal.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn an_initialize_without_a_session_header_is_an_error_before_anything_else_is_sent() {
    let origin = Origin::serving(|_| {
        let recorded = recorded_response("redpine-mcp-initialize.json");
        let mut stripped = Response::new(recorded.status, recorded.body.clone());
        stripped.headers.set("Content-Type", "application/json");
        stripped
    });

    let error = RedpineAdapter::new(&origin.url(), "test-key")
        .quote("NVIDIA", 3)
        .expect_err("no session, no quote");
    assert!(matches!(error, SupplyError::Malformed { .. }), "{error:?}");
    assert_eq!(
        origin.requests().len(),
        1,
        "nothing was sent after the broken handshake"
    );
}

/// The gateway refuses with its own words — a JSON-RPC error object for a
/// protocol fault, an `isError` tool result for a tool fault (both documented
/// shapes; `tools/list` without a session answered "Missing session ID" during
/// reconnaissance). Either must arrive as a status error carrying those
/// words, never as an empty result set that reads like "nothing matched".
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn a_provider_refusal_is_carried_with_its_own_explanation() {
    let origin = Origin::serving(|request| {
        let body: Value = serde_json::from_slice(&request.body).expect("JSON request");
        if body["method"] == "initialize" {
            return recorded_response("redpine-mcp-initialize.json");
        }
        Response::json(
            200,
            r#"{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"Missing session ID — call initialize first"}],"isError":true}}"#,
        )
    });

    let error = RedpineAdapter::new(&origin.url(), "test-key")
        .quote("NVIDIA", 3)
        .expect_err("an error result is not a successful quote");
    match error {
        SupplyError::Status { detail, .. } => assert!(
            detail.contains("Missing session ID"),
            "the provider's own reason must survive: {detail}"
        ),
        other => panic!("expected a status error, got {other:?}"),
    }
}

/// A JSON-RPC `error` object (the protocol's own refusal shape) surfaces the
/// same way.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn a_json_rpc_error_object_is_a_status_error_with_the_provider_message() {
    let origin = Origin::serving(|request| {
        let body: Value = serde_json::from_slice(&request.body).expect("JSON request");
        if body["method"] == "initialize" {
            return recorded_response("redpine-mcp-initialize.json");
        }
        Response::json(
            200,
            r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"Bad Request: no valid session"}}"#,
        )
    });

    let error = RedpineAdapter::new(&origin.url(), "test-key")
        .quote("NVIDIA", 3)
        .expect_err("a JSON-RPC error is not a successful quote");
    match error {
        SupplyError::Status { detail, .. } => assert!(
            detail.contains("no valid session"),
            "the provider's own message must survive: {detail}"
        ),
        other => panic!("expected a status error, got {other:?}"),
    }
}

/// A confirm receipt whose `content` string is not readable JSON is an error,
/// not an empty acquisition: silently returning no envelopes would make a
/// broken parser look like a licensed provider that found nothing.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn an_unreadable_confirm_payload_is_an_error_rather_than_an_empty_result() {
    let origin = Origin::serving(|request| {
        let body: Value = serde_json::from_slice(&request.body).expect("JSON request");
        match (body["method"].as_str(), body["params"]["name"].as_str()) {
            (Some("initialize"), _) => recorded_response("redpine-mcp-initialize.json"),
            (_, Some("get_balance")) => recorded_response("redpine-balance-before.json"),
            (_, Some("inspect-tool")) => recorded_response("redpine-inspect-sample-2.json"),
            (_, Some("preview")) => recorded_response("redpine-preview-sample.json"),
            (_, Some("confirm")) => Response::json(
                200,
                r#"{"jsonrpc":"2.0","id":5,"result":{"content":[{"type":"text","text":"ok"}],"isError":false,"structuredContent":{"preview_id":"pv_8d491df883034fec","tool":"media--sample","cost_charged":"0.000000","balance_remaining":"0.00","content":"<html>not json</html>"}}}"#,
            ),
            other => panic!("unexpected leg {other:?}"),
        }
    });
    let adapter = RedpineAdapter::new(&origin.url(), "test-key");

    let quote = adapter.quote("NVIDIA", 3).expect("recorded quote");
    let error = adapter
        .confirm(&quote)
        .expect_err("an unreadable payload must fail");
    assert!(matches!(error, SupplyError::Malformed { .. }), "{error:?}");
}

/// An unconfigured provider produces a named credential gap, and never a
/// request. No other test in this binary reads or writes the environment.
#[test]
fn a_missing_credential_names_the_variable_and_sends_nothing() {
    unsafe { std::env::remove_var("REDPINE_API_KEY") };

    let error = commonmeasure_supply::supplier_from_environment("redpine")
        .err()
        .expect("an unconfigured provider must not build an adapter");
    assert_eq!(
        error,
        SupplyError::CredentialMissing {
            variable: "REDPINE_API_KEY".to_owned()
        }
    );
}

/// A confirm receipt whose unlocked payload is valid JSON but carries
/// no `mentions` array is a changed shape, not a licence that matched nothing;
/// the error names the field and no acquisition (so no observed charge) exists.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the recorded provider responses: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn a_confirm_payload_without_mentions_is_an_error_naming_the_field() {
    let origin = Origin::serving(|request| {
        let body: Value = serde_json::from_slice(&request.body).expect("JSON request");
        match (body["method"].as_str(), body["params"]["name"].as_str()) {
            (Some("initialize"), _) => recorded_response("redpine-mcp-initialize.json"),
            (_, Some("get_balance")) => recorded_response("redpine-balance-before.json"),
            (_, Some("inspect-tool")) => recorded_response("redpine-inspect-sample-2.json"),
            (_, Some("preview")) => recorded_response("redpine-preview-sample.json"),
            (_, Some("confirm")) => Response::json(
                200,
                r#"{"jsonrpc":"2.0","id":5,"result":{"content":[{"type":"text","text":"ok"}],"isError":false,"structuredContent":{"preview_id":"pv_8d491df883034fec","tool":"media--sample","cost_charged":"0.000000","balance_remaining":"0.00","content":"{}"}}}"#,
            ),
            other => panic!("unexpected leg {other:?}"),
        }
    });
    let adapter = RedpineAdapter::new(&origin.url(), "test-key");

    let quote = adapter.quote("NVIDIA", 3).expect("recorded quote");
    let error = adapter
        .confirm(&quote)
        .expect_err("a payload without `mentions` must fail");
    assert!(matches!(error, SupplyError::Malformed { .. }), "{error:?}");
    assert!(error.to_string().contains("mentions"), "{error}");
}
