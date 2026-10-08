//! People Inc Content API v1, built to the integration guide retrieved on
//! 7 October 2026. Search returns full markdown; no second retrieval is made.
//!
//! Both operations use the production HTTP transport directly because this
//! supplier's errors need response headers as well as RFC 9457 bodies. The
//! shared `execute` success-only helper discards those headers on errors.
//! There are no automatic retries or conditional requests in this slice:
//! each operation sends one request, returns Retry-After to the caller on
//! failure, and refuses redirects and unsolicited 304 responses.

use commonmeasure_http::{Request, Response};
use commonmeasure_types::canonical::sha256_digest;
use commonmeasure_types::{
    AcquisitionCharge, ContextEnvelope, DeclaredDate, LicenceState, ProviderCapability,
};
use serde_json::{Value, json};

use crate::{Acquisition, SupplyAdapter, SupplyError, host_of, parse_json, results_array};

pub(crate) const DEFAULT_BASE_URL: &str = "https://contentmarketplace.people.inc";
pub(crate) const CAPABILITIES: &[ProviderCapability] =
    &[ProviderCapability::Search, ProviderCapability::Fetch];
const MAX_RESULTS: u32 = 5;

/// Search and named-URL retrieval from People Inc's licensed corpus.
/// A corpus description is not a machine-readable licence grant: rights and
/// charges remain unknown, and this adapter declares no `licensed` capability.
pub struct PeopleIncAdapter {
    base_url: String,
    api_key: String,
}

impl PeopleIncAdapter {
    /// `base_url` can name a loopback origin for documented-shape tests.
    /// The credential is held privately and sent only in `X-API-Key`.
    pub fn new(base_url: &str, api_key: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key: api_key.to_owned(),
        }
    }

    fn redact(&self, text: &str) -> String {
        if self.api_key.is_empty() {
            return text.to_owned();
        }
        let escaped = serde_json::to_string(&self.api_key).expect("string serialisation");
        text.replace(&escaped[1..escaped.len() - 1], "«redacted»")
            .replace(&self.api_key, "«redacted»")
    }

    fn exchange(
        &self,
        endpoint: &str,
        mut request: Request,
    ) -> Result<(Response, u64), SupplyError> {
        if self.api_key.trim().is_empty() {
            return Err(SupplyError::CredentialMissing {
                variable: "PEOPLEINC_API_KEY".into(),
            });
        }
        request.headers.set("X-API-Key", &self.api_key);
        request.headers.set("Accept", "application/json");
        request.headers.set("Accept-Encoding", "identity");
        let started = std::time::Instant::now();
        let response = commonmeasure_http::send(endpoint, request).map_err(|error| {
            SupplyError::Transport {
                detail: self.redact(&format!("{error:#}")),
            }
        })?;
        let latency_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
        if response.status != 200 {
            // Keep the whole Problem Details object, including validation
            // errors and requestId. A short generic excerpt can lose them.
            let problem = serde_json::from_slice::<Value>(&response.body).unwrap_or_else(|_| {
                Value::String(String::from_utf8_lossy(&response.body).into_owned())
            });
            let detail = json!({
                "problem": problem,
                "retry_after": response.headers.get("Retry-After"),
                "location_candidate": response.headers.get("Location"),
                "attempts": 1,
                "automatic_retry": false,
                "explanation": match response.status {
                    304 => "unsolicited 304: no authorised cached body exists",
                    300..=399 | 404 => "a moved article or redirect requires confirmation; no substitute was fetched",
                    403 => "article is not licensed; no fallback was attempted",
                    429 => "wait for Retry-After before a caller-authorised retry",
                    _ => "request failed; no automatic retry was attempted",
                },
            });
            return Err(SupplyError::Status {
                status: response.status,
                endpoint: endpoint.to_owned(),
                detail: self.redact(&detail.to_string()),
            });
        }
        // Evidence must retain exact served bytes. If a supplier echoes the
        // key, refuse the body rather than seal a secret or alter the bytes
        // and claim they were served that way.
        let text = String::from_utf8_lossy(&response.body);
        let escaped = serde_json::to_string(&self.api_key).expect("string serialisation");
        let parsed_echo = serde_json::from_slice::<Value>(&response.body)
            .is_ok_and(|body| contains_credential(&body, &self.api_key));
        if text.contains(&self.api_key)
            || text.contains(&escaped[1..escaped.len() - 1])
            || parsed_echo
        {
            return Err(SupplyError::Malformed {
                detail: "People Inc response echoed the credential; body was not retained".into(),
            });
        }
        Ok((response, latency_ms))
    }

    fn acquisition(
        &self,
        capability: ProviderCapability,
        endpoint: String,
        response: Response,
        latency_ms: u64,
        body: &Value,
        envelopes: Vec<ContextEnvelope>,
    ) -> Acquisition {
        Acquisition {
            provider: self.provider().into(),
            capability,
            endpoint,
            http_status: Some(response.status),
            latency_ms,
            provider_request_id: body
                .get("requestId")
                .and_then(Value::as_str)
                .map(str::to_owned),
            charge: AcquisitionCharge::default(),
            envelopes,
            raw_response: response.body,
            invocation: None,
        }
    }
}

impl SupplyAdapter for PeopleIncAdapter {
    fn provider(&self) -> &str {
        "peopleinc"
    }
    fn capabilities(&self) -> &'static [ProviderCapability] {
        CAPABILITIES
    }
    fn maximum_search_results(&self) -> Option<u32> {
        Some(MAX_RESULTS)
    }

    fn search(
        &self,
        query: &str,
        limit: u32,
        _include_hosts: &[&str],
    ) -> Result<Acquisition, SupplyError> {
        let question = query.trim();
        if question.is_empty() || question.chars().count() > 2000 {
            return Err(SupplyError::Malformed {
                detail: "People Inc question must contain 1–2000 characters after trimming".into(),
            });
        }
        let endpoint = format!("{}/v1/search", self.base_url);
        // People Inc documents no host filter. Admission applies the job's
        // host policy to every returned URL, never to its domain label.
        let request = Request::post(
            "/",
            serde_json::to_vec(&json!({
                "question": question, "limit": limit.clamp(1, MAX_RESULTS), "strategy": "balanced",
            }))
            .expect("JSON request serialisation"),
            "application/json",
        );
        let (response, latency_ms) = self.exchange(&endpoint, request)?;
        let body = parse_json(&response.body)?;
        let documents = results_array(&body, "/documents", &endpoint)?;
        if documents.len() > limit.clamp(1, MAX_RESULTS) as usize {
            return Err(SupplyError::Malformed {
                detail: "People Inc returned more documents than the requested limit".into(),
            });
        }
        let envelopes = documents
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let url = item
                    .get("url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| malformed("documents[].url must be a string"))?;
                validate_target(url)?;
                let content = item
                    .get("content")
                    .ok_or_else(|| malformed("documents[].content is missing"))?;
                let mut native = item.clone();
                native
                    .as_object_mut()
                    .ok_or_else(|| malformed("document must be an object"))?
                    .remove("content");
                native
                    .as_object_mut()
                    .expect("document object")
                    .remove("url");
                envelope(url, content, index as u32 + 1, native)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(self.acquisition(
            ProviderCapability::Search,
            endpoint,
            response,
            latency_ms,
            &body,
            envelopes,
        ))
    }

    fn fetch(&self, target: &str) -> Result<Acquisition, SupplyError> {
        validate_target(target)?;
        let mut endpoint = url::Url::parse(&format!("{}/v1/markdown", self.base_url))
            .map_err(|_| malformed("People Inc base URL is invalid"))?;
        // Standard form query encoding; retain the caller's exact identity,
        // including the trailing slash used in the guide's search examples.
        endpoint.query_pairs_mut().append_pair("url", target);
        let endpoint = endpoint.to_string();
        let (response, latency_ms) = self.exchange(&endpoint, Request::get("/"))?;
        let body = parse_json(&response.body)?;
        if body.get("status") != Some(&json!(200)) {
            return Err(malformed(
                "People Inc markdown response must declare status 200",
            ));
        }
        let envelopes = vec![envelope(target, &body, 1, json!({}))?];
        Ok(self.acquisition(
            ProviderCapability::Fetch,
            endpoint,
            response,
            latency_ms,
            &body,
            envelopes,
        ))
    }
}

// A JSON escape can hide an echo from a byte scan while the parser would
// expose it in native metadata. Check both representations before sealing.
fn contains_credential(value: &Value, credential: &str) -> bool {
    match value {
        Value::String(text) => text.contains(credential),
        Value::Array(items) => items
            .iter()
            .any(|item| contains_credential(item, credential)),
        Value::Object(fields) => fields.iter().any(|(name, value)| {
            name.contains(credential) || contains_credential(value, credential)
        }),
        _ => false,
    }
}

fn malformed(detail: &str) -> SupplyError {
    SupplyError::Malformed {
        detail: detail.into(),
    }
}

/// Validate without changing identity. The guide forbids query strings and
/// trailing slashes but shows slashes in its search results: accept a slash
/// unchanged, refuse queries rather than silently removing them.
fn validate_target(target: &str) -> Result<(), SupplyError> {
    let parsed = url::Url::parse(target)
        .map_err(|_| malformed("People Inc target must be a fully-qualified HTTPS URL"))?;
    if !target.starts_with("https://")
        || target.chars().any(|c| c.is_whitespace() || c == '\\')
        || parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.port().is_some()
    {
        return Err(malformed(
            "People Inc target must be HTTPS without credentials, a non-default port, query or fragment; no canonical substitution is made",
        ));
    }
    Ok(())
}

fn envelope(
    url: &str,
    content: &Value,
    rank: u32,
    mut native: Value,
) -> Result<ContextEnvelope, SupplyError> {
    let markdown = content
        .get("markdown")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| malformed("People Inc content.markdown must be a non-empty string"))?;
    let metadata = content.get("metadata");
    if metadata.is_some_and(|value| !value.is_object()) {
        return Err(malformed(
            "People Inc metadata must be an object when present",
        ));
    }
    let field = |name: &str| {
        metadata
            .and_then(|value| value.get(name))
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
    };
    let declared_date = ["pubdate", "update"].into_iter().find_map(|name| {
        field(name).map(|date| DeclaredDate {
            date: date.into(),
            provenance: format!("supplier-declared: People Inc metadata.{name}"),
        })
    });
    if let Some(metadata) = metadata {
        native["metadata"] = metadata.clone();
    }
    Ok(ContextEnvelope {
        source_url: url.into(),
        host: host_of(url).ok_or_else(|| malformed("People Inc URL has no policy host"))?,
        title: field("title").map(str::to_owned),
        text: Some(markdown.into()),
        content_hash: Some(sha256_digest(markdown.as_bytes())),
        licence: LicenceState::Unknown,
        declared_date,
        native_metadata: native,
        retrieval_rank: rank,
    })
}
