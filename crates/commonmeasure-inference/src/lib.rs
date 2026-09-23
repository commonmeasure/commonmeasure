//! The inference-gateway boundary.
//!
//! Common Measure supplies the context and the pinned model plan; an external
//! gateway owns provider compatibility and failover. Local gateways hold their
//! own credentials; direct OpenRouter access uses a scoped bearer key. This
//! crate is the seam, and it is deliberately thin: it constructs an OpenAI-compatible
//! request, sends it over [`commonmeasure_http`], and reports what came back.
//!
//! Two rules govern everything here. A field the gateway did not report stays
//! `None` and is named in [`InferenceResponse::unavailable_fields`] — or, if
//! it arrived in a shape this runtime does not read, in
//! [`InferenceResponse::unreadable_fields`] — never
//! defaulted to zero. An executed route is only ever what the gateway said it
//! executed, never the route that was requested. Those are the two ways an
//! inference record quietly starts lying.
//!
//! There is no backend that answers without a gateway. When none is configured,
//! [`backend_from_environment`] returns `None` and the caller records an
//! explicit unavailable result.

use std::time::Instant;

use commonmeasure_http::Request;
use commonmeasure_types::ModelPlan;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Environment variable naming the gateway's OpenAI-compatible chat-completions
/// endpoint, for example
/// `http://127.0.0.1:3000/openai/v1/chat/completions`.
pub const ENDPOINT_VARIABLE: &str = "COMMONMEASURE_INFERENCE_ENDPOINT";

/// The only destination to which this client sends an OpenRouter key.
pub const OPENROUTER_ENDPOINT: &str = "https://openrouter.ai/api/v1/chat/completions";

/// One source as it is presented to the model, with the hash of the exact text
/// sent.
///
/// The hash is of the text that entered the window, which is the transform
/// stage's output rather than the provider's sealed bytes. A reviewer confirms
/// it in two steps: recompute this hash over `text`, then read the transform's
/// invocation record, which carries that same hash as its output for this
/// source and the hash of the retrieved text as its input. Recomputing in one
/// step over the sealed provider response mismatches whenever a transform
/// changed anything, which is the ordinary case and not evidence of tampering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextPart {
    pub source_ref: String,
    pub content_hash: String,
    pub text: String,
}

/// One context part exactly as the model's window presents it: the labelled
/// header line binding URL and content hash, then the text.
///
/// Public because this layout is one half of the citation seam. The grounding
/// evaluator in `commonmeasure-runtime` teaches a citation form and then parses URLs a
/// model copies out of this header; on 4 August 2026 a live model copied the
/// whole header — `SOURCE` label, hash and all — into its citations, and the
/// evaluator published four admitted sources as outside the window. The seam
/// test in `commonmeasure-runtime`'s evaluator renders parts through this function
/// rather than rebuilding the layout by hand, so the two constants can no
/// longer drift into collision unseen.
pub fn window_block(part: &ContextPart) -> String {
    format!(
        "SOURCE {} [{}]\n{}",
        part.source_ref, part.content_hash, part.text
    )
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InferenceRequest {
    pub run_id: Uuid,
    pub model_plan: ModelPlan,
    pub system: String,
    pub prompt: String,
    pub context: Vec<ContextPart>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InferenceResponse {
    pub output: String,
    pub requested_model: String,
    /// What the gateway said it ran. `None` means it did not say, which is not
    /// the same as having run the requested model.
    pub executed_model: Option<String>,
    pub executed_provider: Option<String>,
    pub route_reason: String,
    /// Wall-clock time measured around the exchange by this runtime. Always
    /// observed, because this is the one number the caller sees for itself.
    pub latency_ms: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub provider_request_id: Option<String>,
    /// Names of the fields above the gateway did not report at all, so an
    /// absence is legible in the artefact without a reader having to notice a
    /// null.
    #[serde(default)]
    pub unavailable_fields: Vec<String>,
    /// Names of the fields the gateway did report in a shape this runtime does
    /// not read — a fractional token count, a model that is not a string. The
    /// value is still `None` above, but the gateway is not silent about it,
    /// and telling an operator it was not reported would send them to the
    /// wrong side of the problem.
    #[serde(default)]
    pub unreadable_fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferenceError {
    /// The request could not be built from this model plan.
    InvalidRequest(String),
    /// No gateway is configured, or it could not be reached.
    Unavailable(String),
    /// The gateway answered with something that is not a usable completion.
    Protocol(String),
}

impl std::fmt::Display for InferenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(detail) => write!(f, "invalid inference request: {detail}"),
            Self::Unavailable(detail) => write!(f, "inference unavailable: {detail}"),
            Self::Protocol(detail) => write!(f, "inference protocol error: {detail}"),
        }
    }
}

impl std::error::Error for InferenceError {}

pub trait InferenceBackend {
    fn name(&self) -> &'static str;
    /// The endpoint this backend will call, for the run record. Never carries a
    /// credential: this string is published verbatim. Authentication belongs
    /// in a scoped header or on the local gateway. An implementation enforces
    /// that when it is constructed, as [`TensorZeroBackend::new`] does.
    fn endpoint(&self) -> &str;
    fn infer(&self, request: &InferenceRequest) -> Result<InferenceResponse, InferenceError>;
}

/// The configured backend, or `None` when the operator has not configured a
/// usable one.
///
/// `None` is a first-class answer. The caller records an unavailable inference
/// result and an explicit gap; there is no weaker backend to fall back to. An
/// endpoint [`TensorZeroBackend::new`] refuses is also `None`, warned about on
/// stderr because the run record cannot tell a refused endpoint from an unset
/// one and the operator who set it needs to know which they have.
pub fn backend_from_environment() -> Option<Box<dyn InferenceBackend>> {
    let endpoint = std::env::var(ENDPOINT_VARIABLE)
        .ok()
        .filter(|endpoint| !endpoint.trim().is_empty())?;
    let configured = if endpoint == OPENROUTER_ENDPOINT {
        openrouter_key(
            std::env::var("OPENROUTER_API_KEY").ok(),
            std::env::var_os("OPENROUTER_API_KEY_FILE").map(std::path::PathBuf::from),
        )
        .and_then(TensorZeroBackend::openrouter)
    } else {
        TensorZeroBackend::new(endpoint)
    };
    match configured {
        Ok(backend) => Some(Box::new(backend)),
        Err(error) => {
            eprintln!("warning: ignoring {ENDPOINT_VARIABLE}: {error}");
            None
        }
    }
}

/// A TensorZero-compatible gateway reached over its OpenAI-compatible HTTP
/// surface.
pub struct TensorZeroBackend {
    endpoint: String,
    // Never serialised, included in a Debug implementation, or attached to a
    // caller-selected URL. Only `openrouter` constructs an authenticated client.
    bearer: Option<String>,
}

fn validate_key(key: String) -> Result<String, InferenceError> {
    let key = key.trim();
    if key.is_empty() || key.len() > 8192 || !key.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(InferenceError::InvalidRequest(
            "OpenRouter key must be nonempty ASCII without whitespace, at most 8192 bytes".into(),
        ));
    }
    Ok(key.to_owned())
}

fn openrouter_key(
    key: Option<String>,
    file: Option<std::path::PathBuf>,
) -> Result<String, InferenceError> {
    use std::io::Read;
    match (key, file) {
        (Some(key), None) => validate_key(key),
        (None, Some(path)) => {
            let mut bytes = String::new();
            std::fs::File::open(path).and_then(|file| file.take(8193).read_to_string(&mut bytes))
                .map_err(|e| InferenceError::InvalidRequest(format!("OpenRouter key file could not be read ({:?})", e.kind())))?;
            if bytes.len() > 8192 {
                return Err(InferenceError::InvalidRequest("OpenRouter key file exceeds 8192 bytes".into()));
            }
            validate_key(bytes)
        }
        _ => Err(InferenceError::InvalidRequest("Set exactly one of OPENROUTER_API_KEY or OPENROUTER_API_KEY_FILE for direct OpenRouter access".into())),
    }
}

impl TensorZeroBackend {
    /// Refuses an endpoint that could carry a credential.
    ///
    /// Userinfo, a query string and a fragment are the three places a URL can
    /// hide one, and none of them can work as gateway authentication here while
    /// all of them are published: this endpoint is written verbatim into the run
    /// record, and run artefacts are committed. Userinfo and the fragment never
    /// reach the wire at all — [`commonmeasure_http`] builds its request from the host and
    /// the origin-form target — so a secret placed there is leaked for nothing.
    /// A query does travel, which makes it the more expensive leak rather than a
    /// supported way to authenticate. Refusing at construction is what makes the
    /// promise on [`InferenceBackend::endpoint`] true.
    pub fn new(endpoint: impl Into<String>) -> Result<Self, InferenceError> {
        let endpoint = endpoint.into();
        let parsed = url::Url::parse(&endpoint).map_err(|error| {
            InferenceError::InvalidRequest(format!("gateway endpoint is not a URL: {error}"))
        })?;
        let carried = if !parsed.username().is_empty() || parsed.password().is_some() {
            "userinfo"
        } else if parsed.query().is_some() {
            "a query string"
        } else if parsed.fragment().is_some() {
            "a fragment"
        } else {
            return Ok(Self {
                endpoint,
                bearer: None,
            });
        };
        Err(InferenceError::InvalidRequest(format!(
            "gateway endpoint carries {carried}; it is published verbatim in the run record and \
             does not authenticate. Configure the credential on the gateway, or use the \
             dedicated OpenRouter key setting with its fixed endpoint."
        )))
    }

    /// Direct OpenRouter access using the same request builder and response
    /// parser. The credential destination is fixed, HTTPS-only and never taken
    /// from the caller. Redirects are not followed by the transport.
    pub fn openrouter(key: String) -> Result<Self, InferenceError> {
        let key = validate_key(key)?;
        Ok(Self {
            endpoint: OPENROUTER_ENDPOINT.to_owned(),
            bearer: Some(key),
        })
    }
}

impl InferenceBackend for TensorZeroBackend {
    fn name(&self) -> &'static str {
        if self.bearer.is_some() {
            "openrouter"
        } else {
            "tensorzero"
        }
    }

    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn infer(&self, request: &InferenceRequest) -> Result<InferenceResponse, InferenceError> {
        request.model_plan.validate().map_err(|error| {
            InferenceError::InvalidRequest(format!("invalid model plan: {error:?}"))
        })?;

        let body = chat_completions_body(request);
        let encoded = serde_json::to_vec(&body)
            .map_err(|error| InferenceError::InvalidRequest(error.to_string()))?;
        let mut http_request = Request::post("/", encoded, "application/json");
        http_request.headers.set("Accept", "application/json");
        if let Some(key) = &self.bearer {
            http_request
                .headers
                .set("Authorization", &format!("Bearer {key}"));
        }

        let started = Instant::now();
        let response = commonmeasure_http::send(&self.endpoint, http_request).map_err(|error| {
            let detail = format!("{error:#}");
            InferenceError::Unavailable(match &self.bearer {
                Some(key) => detail.replace(key, "[redacted]"),
                None => detail,
            })
        })?;
        let latency_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);

        if self.bearer.as_ref().is_some_and(|key| {
            response
                .body
                .windows(key.len())
                .any(|part| part == key.as_bytes())
        }) {
            return Err(InferenceError::Protocol(
                "gateway response echoed its authentication credential; response omitted".into(),
            ));
        }

        if !(200..300).contains(&response.status) {
            let status = response.status;
            let mut detail = format!("gateway answered {status} for {}", self.endpoint);
            // The gateway's own account of the failure was in hand; without it
            // the evidence gap can only say that inference failed, never that
            // the model was not loaded.
            // Authenticated error bodies are not evidence: an upstream error
            // can echo a credential or a transformed fragment of one.
            let excerpt = if self.bearer.is_some() {
                String::new()
            } else {
                body_excerpt(&response.body)
            };
            if !excerpt.is_empty() {
                detail.push_str(": ");
                detail.push_str(&excerpt);
            }
            return Err(InferenceError::Protocol(detail));
        }
        let parsed: Value = serde_json::from_slice(&response.body).map_err(|error| {
            InferenceError::Protocol(format!("gateway response is not valid JSON: {error}"))
        })?;

        let output = parsed
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                InferenceError::Protocol("missing choices[0].message.content".to_owned())
            })?
            .to_owned();

        let executed_model = parsed
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let executed_provider = parsed
            .pointer("/choices/0/message/provider")
            .or_else(|| parsed.get("provider"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let input_tokens = parsed
            .pointer("/usage/prompt_tokens")
            .and_then(Value::as_u64);
        let output_tokens = parsed
            .pointer("/usage/completion_tokens")
            .and_then(Value::as_u64);
        let provider_request_id = parsed.get("id").and_then(Value::as_str).map(str::to_owned);

        // A field the gateway reported in a shape this runtime does not read
        // is not a field the gateway withheld. The two are named apart, so an
        // operator chasing a missing token count is sent to the gateway that
        // reported a fraction rather than to one that reported nothing.
        let reported = |pointer: &str| {
            parsed
                .pointer(pointer)
                .filter(|value| !value.is_null())
                .is_some()
        };
        let mut unavailable_fields = Vec::new();
        let mut unreadable_fields = Vec::new();
        for (name, read, reported) in [
            (
                "executed_model",
                executed_model.is_some(),
                reported("/model"),
            ),
            (
                "executed_provider",
                executed_provider.is_some(),
                reported("/choices/0/message/provider") || reported("/provider"),
            ),
            (
                "input_tokens",
                input_tokens.is_some(),
                reported("/usage/prompt_tokens"),
            ),
            (
                "output_tokens",
                output_tokens.is_some(),
                reported("/usage/completion_tokens"),
            ),
            (
                "provider_request_id",
                provider_request_id.is_some(),
                reported("/id"),
            ),
        ] {
            match (read, reported) {
                (true, _) => {}
                (false, true) => unreadable_fields.push(name.to_owned()),
                (false, false) => unavailable_fields.push(name.to_owned()),
            }
        }

        Ok(InferenceResponse {
            output,
            requested_model: request.model_plan.model.clone(),
            executed_model,
            executed_provider,
            route_reason: if self.bearer.is_some() {
                "model requested through OpenRouter; provider routing and fallbacks use the account defaults; only reported execution fields establish the route"
            } else {
                "pinned by the job's model plan; no gateway fallback requested"
            }.to_owned(),
            latency_ms,
            input_tokens,
            output_tokens,
            provider_request_id,
            unavailable_fields,
            unreadable_fields,
        })
    }
}

/// What a failing gateway said, short enough to belong in an error.
///
/// Bounded because a gateway that is not answering JSON may answer with a proxy
/// error page, and a run record is no place for one.
fn body_excerpt(body: &[u8]) -> String {
    const LIMIT: usize = 200;
    let text = String::from_utf8_lossy(body);
    let text = text.trim();
    if text.len() <= LIMIT {
        return text.to_owned();
    }
    let mut end = LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// The digest of the request body this runtime builds, taken over a fixed
/// probe.
///
/// The run manifest seals it, so a change to [`chat_completions_body`] — the
/// sampling temperature, the way a source is rendered into the window, a
/// parameter added or dropped — publishes a different experiment. Two runs
/// that put the same sources in front of the same model in different words
/// are not one comparison, and without this the manifest hash could not tell
/// them apart.
///
/// Nothing about a real job enters the probe. The job, the composed system
/// prompt, the model plan and the admitted window are sealed on their own;
/// what this adds is the shape they are sent in.
#[must_use]
pub fn request_shape_digest() -> String {
    let probe = InferenceRequest {
        run_id: Uuid::nil(),
        model_plan: ModelPlan {
            name: "probe".to_owned(),
            version: "1".to_owned(),
            model: "probe-model".to_owned(),
        },
        system: "SYSTEM".to_owned(),
        prompt: "PROMPT".to_owned(),
        context: vec![ContextPart {
            source_ref: "https://probe.example/a".to_owned(),
            content_hash: "sha256:probe".to_owned(),
            text: "TEXT".to_owned(),
        }],
    };
    commonmeasure_types::canonical::canonical_digest(&chat_completions_body(&probe))
}

/// The OpenAI-compatible request body.
///
/// `temperature: 0` and `stream: false` are experimental controls, not
/// preferences: a comparison that varies sampling is not comparing context
/// plans. The run identity travels as request metadata so a gateway's own
/// observability can be joined to this run's evidence.
fn chat_completions_body(request: &InferenceRequest) -> Value {
    let context = request
        .context
        .iter()
        .map(window_block)
        .collect::<Vec<_>>()
        .join("\n\n");
    let user = if context.is_empty() {
        format!("{}\n\nSUPPLIED CONTEXT\n(none)", request.prompt)
    } else {
        format!("{}\n\nSUPPLIED CONTEXT\n{context}", request.prompt)
    };
    serde_json::json!({
        "model": request.model_plan.model,
        "messages": [
            {"role": "system", "content": request.system},
            {"role": "user", "content": user},
        ],
        "temperature": 0,
        "stream": false,
        "metadata": {"commonmeasure_run_id": request.run_id.to_string()},
    })
}

#[cfg(test)]
mod openrouter_tests {
    use super::*;
    use commonmeasure_http::{Response, Server};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    const KEY: &str = "fixture-openrouter-secret-not-a-real-key";

    fn request() -> InferenceRequest {
        InferenceRequest {
            run_id: Uuid::new_v4(),
            model_plan: ModelPlan {
                name: "benchmark-answer".into(),
                version: "1".into(),
                model: "openai/gpt-4.1".into(),
            },
            system: "Use the supplied context".into(),
            prompt: "A fixture question".into(),
            context: vec![],
        }
    }

    #[test]
    fn authenticated_http_keeps_key_out_of_body_and_preserves_executed_route() {
        let calls = Arc::new(AtomicUsize::new(0));
        let received = calls.clone();
        let server = Server::bind("127.0.0.1:0").unwrap().spawn(move |request| {
            assert_eq!(request.headers.get("Authorization"), Some(format!("Bearer {KEY}").as_str()));
            assert!(!String::from_utf8_lossy(&request.body).contains(KEY));
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["model"], "openai/gpt-4.1");
            assert_eq!(body["temperature"], 0);
            received.fetch_add(1, Ordering::SeqCst);
            Response::json(200, r#"{"id":"generation-fixture","model":"openai/gpt-4.1-2025-04-14","provider":"OpenAI","choices":[{"message":{"content":"Fixture answer"}}],"usage":{"prompt_tokens":20,"completion_tokens":3}}"#)
        }).unwrap();
        let mut backend = TensorZeroBackend::openrouter(KEY.into()).unwrap();
        assert_eq!(backend.endpoint(), OPENROUTER_ENDPOINT);
        assert_eq!(backend.name(), "openrouter");
        // Private test-only substitution. The public constructor cannot send
        // an OpenRouter key to a caller-supplied destination.
        backend.endpoint = server.url();
        let response = backend.infer(&request()).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            response.executed_model.as_deref(),
            Some("openai/gpt-4.1-2025-04-14")
        );
        assert_eq!(response.executed_provider.as_deref(), Some("OpenAI"));
        assert!(!serde_json::to_string(&response).unwrap().contains(KEY));
    }

    #[test]
    fn redirects_do_not_forward_credentials_and_error_bodies_are_omitted() {
        let hits = Arc::new(AtomicUsize::new(0));
        let redirected = hits.clone();
        let target = Server::bind("127.0.0.1:0")
            .unwrap()
            .spawn(move |_| {
                redirected.fetch_add(1, Ordering::SeqCst);
                Response::json(200, "{}")
            })
            .unwrap();
        let location = target.url();
        let origin = Server::bind("127.0.0.1:0")
            .unwrap()
            .spawn(move |_| {
                let mut response =
                    Response::json(302, r#"{"message":"private authentication diagnostic"}"#);
                response.headers.set("Location", &location);
                response
            })
            .unwrap();
        let mut backend = TensorZeroBackend::openrouter(KEY.into()).unwrap();
        backend.endpoint = origin.url();
        let error = backend.infer(&request()).unwrap_err().to_string();
        assert!(error.contains("302"));
        assert!(!error.contains("private authentication diagnostic"));
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn echoed_credentials_are_rejected_even_in_successful_responses() {
        let server = Server::bind("127.0.0.1:0")
            .unwrap()
            .spawn(|_| {
                Response::json(
                    200,
                    &serde_json::json!({"choices":[{"message":{"content":KEY}}]}).to_string(),
                )
            })
            .unwrap();
        let mut backend = TensorZeroBackend::openrouter(KEY.into()).unwrap();
        backend.endpoint = server.url();
        let error = backend.infer(&request()).unwrap_err().to_string();
        assert!(!error.contains(KEY));
        assert!(error.contains("response omitted"));
    }

    #[test]
    fn credential_loading_is_exclusive_bounded_and_never_quotes_the_value() {
        for key in ["", "injected\r\nX-Secret: value", "embedded whitespace"] {
            assert!(TensorZeroBackend::openrouter(key.into()).is_err());
        }
        assert!(openrouter_key(None, None).is_err());
        let file = std::env::temp_dir().join(format!("openrouter-test-{}", Uuid::new_v4()));
        std::fs::write(&file, format!("{KEY}\n")).unwrap();
        assert_eq!(openrouter_key(None, Some(file.clone())).unwrap(), KEY);
        let error = openrouter_key(Some(KEY.into()), Some(file.clone()))
            .unwrap_err()
            .to_string();
        assert!(!error.contains(KEY));
        std::fs::write(&file, vec![b'x'; 8193]).unwrap();
        assert!(openrouter_key(None, Some(file.clone())).is_err());
        std::fs::remove_file(file).unwrap();
        assert!(
            TensorZeroBackend::new("http://127.0.0.1:3000")
                .unwrap()
                .bearer
                .is_none()
        );
    }
}
