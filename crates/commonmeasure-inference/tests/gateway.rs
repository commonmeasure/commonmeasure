//! The gateway boundary against a real HTTP gateway on loopback.
//!
//! There is no `TensorZeroTransport` to implement in a test, because there is
//! no transport seam: the backend speaks HTTP, so a test that wants to control
//! what the gateway says starts one. The request assertions below are on bytes
//! that crossed a socket.

use commonmeasure_http::{Request, Response, Server, ServerHandle};
use commonmeasure_inference::{
    ContextPart, ENDPOINT_VARIABLE, InferenceBackend, InferenceError, InferenceRequest,
    TensorZeroBackend, backend_from_environment, window_block,
};
use commonmeasure_types::ModelPlan;
use serde_json::Value;
use std::sync::{Arc, Mutex, MutexGuard};
use uuid::Uuid;

/// A credential-free endpoint on a port this repository reserves for tests.
/// Nothing listens on it: these cases are decided before a socket is opened.
const ORDINARY_ENDPOINT: &str = "http://127.0.0.1:47450/v1/chat/completions";

/// The endpoints an operator might reach for to authenticate a gateway, one per
/// place a URL can hide a secret.
const CREDENTIALLED_ENDPOINTS: [&str; 3] = [
    "http://user:pass@127.0.0.1:47450/v1/chat/completions",
    "http://127.0.0.1:47450/v1/chat/completions?api_key=x",
    "http://127.0.0.1:47450/v1/chat/completions#api_key=x",
];

/// `ENDPOINT_VARIABLE` is process-wide and a test binary is threaded, so the
/// tests that read it take turns; and each restores what it found, because the
/// documented way to run this product live is to have the variable set.
static ENDPOINT_LOCK: Mutex<()> = Mutex::new(());

struct ScopedEndpoint {
    _turn: MutexGuard<'static, ()>,
    restore: Option<String>,
}

impl ScopedEndpoint {
    fn set(value: &str) -> Self {
        Self::apply(Some(value))
    }

    fn unset() -> Self {
        Self::apply(None)
    }

    fn apply(value: Option<&str>) -> Self {
        let turn = ENDPOINT_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let restore = std::env::var(ENDPOINT_VARIABLE).ok();
        // Sound because `turn` is held for as long as this guard lives, so no
        // other test in this binary is reading or writing the environment.
        unsafe { write_endpoint(value) };
        Self {
            _turn: turn,
            restore,
        }
    }
}

impl Drop for ScopedEndpoint {
    fn drop(&mut self) {
        unsafe { write_endpoint(self.restore.as_deref()) };
    }
}

unsafe fn write_endpoint(value: Option<&str>) {
    unsafe {
        match value {
            Some(value) => std::env::set_var(ENDPOINT_VARIABLE, value),
            None => std::env::remove_var(ENDPOINT_VARIABLE),
        }
    }
}

fn gateway<H>(handler: H) -> (ServerHandle, Arc<Mutex<Vec<Value>>>)
where
    H: Fn() -> Response + Send + Sync + 'static,
{
    let seen: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&seen);
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |request: Request| {
            if let Ok(body) = serde_json::from_slice::<Value>(&request.body) {
                recorder.lock().expect("seen lock").push(body);
            }
            handler()
        })
        .expect("spawn");
    (handle, seen)
}

fn request(context: Vec<ContextPart>) -> InferenceRequest {
    InferenceRequest {
        run_id: Uuid::new_v4(),
        model_plan: ModelPlan {
            name: "fixed".into(),
            version: "1".into(),
            model: "test-model".into(),
        },
        system: "Use only supplied evidence.".into(),
        prompt: "What does the evidence establish?".into(),
        context,
    }
}

#[test]
fn a_complete_gateway_response_is_read_field_for_field() {
    let (handle, seen) = gateway(|| {
        Response::json(
            200,
            r#"{"id":"tz-1","model":"llama-3.2-3b","provider":"ollama",
                "choices":[{"message":{"content":"The evidence establishes little."}}],
                "usage":{"prompt_tokens":12,"completion_tokens":7}}"#,
        )
    });
    let backend = TensorZeroBackend::new(handle.url()).expect("an ordinary loopback endpoint");

    let response = backend
        .infer(&request(vec![ContextPart {
            source_ref: "https://www.ofgem.gov.uk/energy-price-cap".into(),
            content_hash: "sha256:abc".into(),
            text: "Ofgem publishes the cap.".into(),
        }]))
        .expect("a well-formed gateway response");

    let sent = seen.lock().expect("seen lock")[0].clone();
    assert_eq!(sent["model"], "test-model");
    assert_eq!(sent["temperature"], 0, "sampling is fixed for a comparison");
    assert_eq!(sent["stream"], false);
    assert!(
        sent["metadata"]["commonmeasure_run_id"].as_str().is_some(),
        "the run identity must reach the gateway's own observability"
    );
    let user = sent["messages"][1]["content"].as_str().unwrap();
    assert!(
        user.contains("sha256:abc") && user.contains("Ofgem publishes the cap."),
        "the model must see the source reference and hash beside its text"
    );

    assert_eq!(response.output, "The evidence establishes little.");
    assert_eq!(response.requested_model, "test-model");
    assert_eq!(response.executed_model.as_deref(), Some("llama-3.2-3b"));
    assert_eq!(response.executed_provider.as_deref(), Some("ollama"));
    assert_eq!(response.input_tokens, Some(12));
    assert_eq!(response.output_tokens, Some(7));
    assert_eq!(response.provider_request_id.as_deref(), Some("tz-1"));
    assert!(response.unavailable_fields.is_empty());
}

/// This crate's half of the citation seam: the window the request body
/// carries is `window_block`'s output, byte for byte, part after part. The
/// `SOURCE <url> [<hash>]` header taught a live model what a source token
/// looks like, and for one release no assertion on either side owned that
/// string — `contains` checks on the hash and the text held while the layout
/// around them was free to drift into collision with the citation directive.
/// Asserting byte-equality against the exported production renderer (never a
/// re-derived format string) means a layout change must come through
/// `window_block`, where the evaluator's seam test in commonmeasure-runtime is waiting
/// for it.
#[test]
fn the_request_window_is_window_block_output_byte_for_byte() {
    let (handle, seen) =
        gateway(|| Response::json(200, r#"{"choices":[{"message":{"content":"Answer."}}]}"#));
    let parts = vec![
        ContextPart {
            source_ref: "https://www.ofgem.gov.uk/energy-price-cap".into(),
            content_hash: "sha256:abc".into(),
            text: "Ofgem publishes the cap.".into(),
        },
        ContextPart {
            source_ref: "https://example.org/second".into(),
            content_hash: "sha256:def".into(),
            text: "A second part,\nspanning two lines.".into(),
        },
    ];

    TensorZeroBackend::new(handle.url())
        .expect("an ordinary loopback endpoint")
        .infer(&request(parts.clone()))
        .expect("a well-formed gateway response");

    let sent = seen.lock().expect("seen lock")[0].clone();
    let user = sent["messages"][1]["content"]
        .as_str()
        .expect("a user message crossed the socket");
    for part in &parts {
        assert!(
            user.contains(&window_block(part)),
            "the window must carry the production renderer's block for {}, byte for byte:\n{user}",
            part.source_ref
        );
    }
    // The blocks are the whole window: rendered in order, joined by one blank
    // line, closing the message. Anything after them would be context the
    // record never saw rendered.
    let rendered = parts
        .iter()
        .map(window_block)
        .collect::<Vec<_>>()
        .join("\n\n");
    assert!(
        user.ends_with(&rendered),
        "the supplied context must end with exactly the rendered blocks:\n{user}"
    );
}

/// The finding this test exists for: a gateway that reports no usage and no
/// model must not have its silence turned into zero tokens and a claim that the
/// requested route ran. Both are fabrications, and the second is the more
/// dangerous — it is an execution claim nothing supports.
#[test]
fn a_silent_gateway_yields_unknowns_never_zeroes_or_the_requested_route() {
    let (handle, _) =
        gateway(|| Response::json(200, r#"{"choices":[{"message":{"content":"Answer."}}]}"#));

    let response = TensorZeroBackend::new(handle.url())
        .expect("an ordinary loopback endpoint")
        .infer(&request(Vec::new()))
        .expect("a minimal but valid completion");

    assert_eq!(response.output, "Answer.");
    assert_eq!(response.requested_model, "test-model");
    assert_eq!(
        response.executed_model, None,
        "an unreported executed model must not become the requested model"
    );
    assert_eq!(response.executed_provider, None);
    assert_eq!(response.input_tokens, None, "unknown is not zero");
    assert_eq!(response.output_tokens, None, "unknown is not zero");
    assert_eq!(response.provider_request_id, None);
    for expected in [
        "executed_model",
        "executed_provider",
        "input_tokens",
        "output_tokens",
        "provider_request_id",
    ] {
        assert!(
            response.unavailable_fields.iter().any(|f| f == expected),
            "{expected} is absent and must be named as unavailable"
        );
    }
}

/// A gateway that reports a field in a shape this runtime does not read is
/// not a gateway that reported nothing, and the record says which it was.
///
/// A fractional `prompt_tokens` and a model that is not a string both leave
/// the value unknown, as they must. Filing them under "the gateway did not
/// report" would send an operator to the wrong side of the problem: the
/// gateway did report, and what it sent is what needs looking at.
#[test]
fn a_field_reported_in_an_unreadable_shape_is_not_a_field_the_gateway_withheld() {
    let (handle, _) = gateway(|| {
        Response::json(
            200,
            r#"{"id":42,"model":{"name":"pinned"},
                "choices":[{"message":{"content":"Answer."}}],
                "usage":{"prompt_tokens":40.5,"completion_tokens":9}}"#,
        )
    });

    let response = TensorZeroBackend::new(handle.url())
        .expect("an ordinary loopback endpoint")
        .infer(&request(Vec::new()))
        .expect("an answer arrived; a usage field is no reason to discard it");

    assert_eq!(response.output, "Answer.");
    assert_eq!(
        response.input_tokens, None,
        "a fraction is not a token count"
    );
    assert_eq!(response.output_tokens, Some(9));
    assert_eq!(response.executed_model, None);
    assert_eq!(response.provider_request_id, None);

    let mut unreadable = response.unreadable_fields.clone();
    unreadable.sort();
    assert_eq!(
        unreadable,
        ["executed_model", "input_tokens", "provider_request_id"],
        "each field the gateway sent in a shape this runtime cannot read is named apart"
    );
    assert_eq!(
        response.unavailable_fields,
        ["executed_provider"],
        "only the field the gateway genuinely did not send is unavailable"
    );
}

/// The request body's shape is part of experiment identity, so the digest the
/// manifest seals is pinned here.
///
/// Sampling temperature and the way a source is rendered into the window are
/// experimental controls, not preferences. Changing either changes what the
/// model was asked, so it must change the manifest hash. This test fails when
/// the body changes, which is the point: update the pinned digest only
/// together with a decision that the experiment is a different one.
#[test]
fn the_request_shape_digest_is_pinned_to_the_body_this_runtime_sends() {
    assert_eq!(
        commonmeasure_inference::request_shape_digest(),
        "sha256:69b7c1b68cb95b6742039de389a5e87a152fdfc9bc937939ecd1b24dde3ee466"
    );

    // What the digest is over, stated so a failure above says what moved:
    // the sampling controls, the two message roles, and the window layout.
    let (handle, seen) =
        gateway(|| Response::json(200, r#"{"choices":[{"message":{"content":"Answer."}}]}"#));
    TensorZeroBackend::new(handle.url())
        .expect("an ordinary loopback endpoint")
        .infer(&request(vec![ContextPart {
            source_ref: "https://a.example/page".to_owned(),
            content_hash: "sha256:abc".to_owned(),
            text: "TEXT".to_owned(),
        }]))
        .expect("an answer");
    let seen = seen.lock().expect("seen lock");
    let body = seen.first().expect("one request");
    assert_eq!(
        body["temperature"], 0,
        "a comparison must not vary sampling"
    );
    assert_eq!(body["stream"], false);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][1]["role"], "user");
    assert!(
        body["messages"][1]["content"]
            .as_str()
            .expect("the user message")
            .contains("SOURCE https://a.example/page [sha256:abc]"),
        "the window layout is part of what the model was asked: {}",
        body["messages"][1]["content"]
    );
}

/// A refused status is not an answer, and the gateway's account of why it
/// refused was in hand: an operator reading the evidence gap must be able to
/// see that the model was not loaded, not merely that inference failed.
#[test]
fn a_gateway_error_status_is_a_protocol_failure_carrying_its_stated_cause() {
    let (handle, _) =
        gateway(|| Response::json(503, r#"{"error":{"message":"model not loaded: llama"}}"#));

    let error = TensorZeroBackend::new(handle.url())
        .expect("an ordinary loopback endpoint")
        .infer(&request(Vec::new()))
        .expect_err("a 503 is not a completion");
    match error {
        InferenceError::Protocol(detail) => {
            assert!(detail.contains("503"), "{detail}");
            assert!(detail.contains("model not loaded: llama"), "{detail}");
        }
        other => panic!("expected a protocol error, got {other:?}"),
    }
}

/// An error body is excerpted, not carried whole: a gateway that is not
/// answering JSON may answer with a proxy's error page, and that does not
/// belong in a run's evidence.
#[test]
fn a_long_error_body_reaches_the_error_bounded() {
    let filler = "y".repeat(4000);
    let (handle, _) = gateway(move || Response::json(502, &format!("<html>{filler}</html>")));

    let error = TensorZeroBackend::new(handle.url())
        .expect("an ordinary loopback endpoint")
        .infer(&request(Vec::new()))
        .expect_err("a 502 is not a completion");
    match error {
        InferenceError::Protocol(detail) => {
            assert!(detail.contains("<html>"), "{detail}");
            assert!(
                detail.len() < 400,
                "the excerpt is bounded: {}",
                detail.len()
            );
        }
        other => panic!("expected a protocol error, got {other:?}"),
    }
}

#[test]
fn a_response_without_a_completion_is_refused_rather_than_guessed_at() {
    let (handle, _) = gateway(|| Response::json(200, r#"{"choices":[]}"#));

    let error = TensorZeroBackend::new(handle.url())
        .expect("an ordinary loopback endpoint")
        .infer(&request(Vec::new()))
        .expect_err("no completion means no answer");
    match error {
        InferenceError::Protocol(detail) => assert!(detail.contains("choices")),
        other => panic!("expected a protocol error, got {other:?}"),
    }
}

/// An unreachable gateway is unavailable. There is nothing to fall back to, and
/// the caller turns this into an explicit unavailable plan.
#[test]
fn an_unreachable_gateway_is_unavailable() {
    // Port 0 is not connectable; nothing is listening and nothing will be.
    let error = TensorZeroBackend::new("http://127.0.0.1:1/v1/chat/completions")
        .expect("a credential-free endpoint")
        .infer(&request(Vec::new()))
        .expect_err("an unreachable gateway cannot answer");
    assert!(matches!(error, InferenceError::Unavailable(_)), "{error:?}");
}

#[test]
fn an_empty_model_is_rejected_before_anything_is_sent() {
    let mut request = request(Vec::new());
    request.model_plan.model = "  ".into();
    let error = TensorZeroBackend::new("http://127.0.0.1:1/unused")
        .expect("a credential-free endpoint")
        .infer(&request)
        .expect_err("an empty model plan is not executable");
    assert!(
        matches!(error, InferenceError::InvalidRequest(_)),
        "{error:?}"
    );
}

/// No backend configured is a first-class state, not an error to be smoothed
/// over with a default.
#[test]
fn no_configured_endpoint_yields_no_backend() {
    let _endpoint = ScopedEndpoint::unset();
    assert!(backend_from_environment().is_none());
}

/// A variable set to whitespace is a variable the operator meant to clear.
#[test]
fn a_whitespace_endpoint_is_no_endpoint() {
    let _endpoint = ScopedEndpoint::set("   ");
    assert!(
        backend_from_environment().is_none(),
        "whitespace is not an address to send a run's context to"
    );
}

/// The endpoint is published verbatim into the run summary, and this repository
/// commits run summaries. A credential in it is therefore leaked, and refusing
/// it late — at the first inference — would be after it had been written.
#[test]
fn an_endpoint_carrying_a_credential_is_refused_at_construction() {
    for endpoint in CREDENTIALLED_ENDPOINTS {
        let error = TensorZeroBackend::new(endpoint)
            .err()
            .unwrap_or_else(|| panic!("{endpoint} must not construct a backend"));
        assert!(
            matches!(error, InferenceError::InvalidRequest(_)),
            "{endpoint}: {error:?}"
        );
    }
}

#[test]
fn a_credentialled_endpoint_in_the_environment_yields_no_backend() {
    for endpoint in CREDENTIALLED_ENDPOINTS {
        let _endpoint = ScopedEndpoint::set(endpoint);
        assert!(
            backend_from_environment().is_none(),
            "{endpoint} would put its secret in the run summary"
        );
    }
}

#[test]
fn an_ordinary_endpoint_still_configures_a_backend() {
    let backend = TensorZeroBackend::new(ORDINARY_ENDPOINT).expect("a credential-free endpoint");
    assert_eq!(backend.endpoint(), ORDINARY_ENDPOINT);

    let _endpoint = ScopedEndpoint::set(ORDINARY_ENDPOINT);
    let configured = backend_from_environment().expect("a credential-free endpoint configures");
    assert_eq!(configured.endpoint(), ORDINARY_ENDPOINT);
}
