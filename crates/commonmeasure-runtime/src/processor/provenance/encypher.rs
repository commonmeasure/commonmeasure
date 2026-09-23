//! Optional Encypher text signing. A job authorises disclosure explicitly;
//! returned credentials are checked locally before any label is published.

use super::*;
use commonmeasure_http::Request;

#[cfg(test)]
mod tests;

const ENDPOINT: &str = "https://api.encypher.com/api/v1/sign";
const RULES: &str = "encypher-output-provenance/1: explicit suite disclosure of answer and source \
record; fixed HTTPS /api/v1/sign; answer <=1 MB, request <=2 MiB; request-hash idempotency key; one request, no redirects or retries; full document C2PA \
embedding in plain text; no attribution indexing or manifest database storage requested; \
custom source record, CAWG training-mining and individually indexed standard inputTo \
ingredient assertions; created action \
with trainedAlgorithmicMedia; independently require trusted C2PA/CAWG validation, unchanged \
NFC text, exact source record and training preferences, matching ingredient hashes, grades \
and relationships, and the declared creation action. Failure publishes no label. Provider \
certificate establishes claim signer only. Record request/response hashes, HTTP status, \
trust bundle digest and raw credential artefacts; do not log bodies or credentials. \
Provider cost and complete retention remain unknown. No fallback to local signing.";

/// Permission sealed into the suite before any remote request can be made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Disclosure {
    /// Includes source references, hashes, grades, run identifiers and output
    /// preferences. Prompts and source bodies are not sent by the adapter.
    pub send_answer_and_source_record: bool,
}

/// Credentials and trust material supplied by the operator, never by content.
#[derive(Clone)]
pub struct SigningConfig {
    api_key: String,
    trust_anchors: String,
    #[cfg(test)]
    endpoint: String,
    #[cfg(test)]
    capture_directory: Option<std::path::PathBuf>,
}

impl std::fmt::Debug for SigningConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncypherSigningConfig")
            .field("api_key", &"[redacted]")
            .field(
                "trust_anchors_hash",
                &sha256_digest(self.trust_anchors.as_bytes()),
            )
            .finish()
    }
}

impl SigningConfig {
    /// Load only the named signing key and trust bundle. The credential can
    /// be supplied as an environment value or private file, but never both.
    pub fn from_environment() -> Result<Self, String> {
        let api_key = match (
            std::env::var("ENCYPHER_API_KEY").ok(),
            std::env::var("ENCYPHER_API_KEY_FILE").ok(),
        ) {
            (Some(key), None) => key,
            (None, Some(path)) => std::fs::read_to_string(path)
                .map_err(|_| "ENCYPHER_API_KEY_FILE cannot be read".to_owned())?
                .trim()
                .to_owned(),
            (None, None) => return Err("set ENCYPHER_API_KEY or ENCYPHER_API_KEY_FILE".to_owned()),
            (Some(_), Some(_)) => {
                return Err("set only one of ENCYPHER_API_KEY and ENCYPHER_API_KEY_FILE".to_owned());
            }
        };
        if api_key.is_empty()
            || api_key.len() > 4096
            || !api_key.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err("the Encypher API key has an invalid format".to_owned());
        }
        let path = std::env::var("COMMONMEASURE_PROVENANCE_TRUST_ANCHORS").map_err(|_| {
            "set COMMONMEASURE_PROVENANCE_TRUST_ANCHORS to a local PEM bundle".to_owned()
        })?;
        let trust_anchors = std::fs::read_to_string(path)
            .map_err(|_| "COMMONMEASURE_PROVENANCE_TRUST_ANCHORS cannot be read".to_owned())?;
        validate_trust_bundle(&trust_anchors)
            .map_err(|_| "the signing trust bundle is malformed".to_owned())?;
        Ok(Self {
            api_key,
            trust_anchors,
            #[cfg(test)]
            endpoint: ENDPOINT.to_owned(),
            #[cfg(test)]
            capture_directory: None,
        })
    }

    fn endpoint(&self) -> &str {
        #[cfg(test)]
        {
            &self.endpoint
        }
        #[cfg(not(test))]
        {
            ENDPOINT
        }
    }
}

static MANIFEST: LazyLock<ProcessorManifest> = LazyLock::new(|| {
    let mut manifest = super::manifest().clone();
    manifest.version = "2";
    manifest.configuration_digest = sha256_digest(RULES.as_bytes());
    manifest.permissions.network = true;
    manifest.limits = "one HTTP exchange; 30-second connect/write/read budget excluding host DNS; 32 MiB response ceiling; no redirects or retries";
    manifest
});

/// The selected implementation, sealed in the run in place of the local signer.
pub fn manifest() -> &'static ProcessorManifest {
    &MANIFEST
}

fn request_body(input: &Input) -> Result<Value, &'static str> {
    let mut assertions = vec![
        json!({"label": SOURCE_RECORD_LABEL, "data": source_record(input)}),
        json!({"label": TRAINING_MINING_LABEL, "data": input.policy.training_mining.assertion()}),
    ];
    for (index, source) in input.sources.iter().enumerate() {
        let data = json!({
            "dc:title": source.reference, "dc:format": TEXT_FORMAT,
            "instanceID": source.content_hash, "description": source.grade.as_str(),
            "relationship": "inputTo",
        });
        let label = if index == 0 {
            "c2pa.ingredient.v3".to_owned()
        } else {
            format!("c2pa.ingredient.v3__{index}")
        };
        assertions.push(json!({"label": label, "data": data}));
    }
    let record =
        serde_json::to_vec(&source_record(input)).map_err(|_| "cannot encode source record")?;
    let id = sha256_digest(&record).replace("sha256:", "cm-");
    Ok(json!({
        "text": input.answer,
        "document_id": id,
        "options": {
            "document_type": "ai_output",
            "action": "c2pa.created",
            "digital_source_type": DIGITAL_SOURCE_TYPE,
            "segmentation_level": "document",
            "manifest_mode": "full",
            "embedding_strategy": "single_point",
            "embedding_options": {"format": "plain", "method": "invisible", "include_text": true},
            "custom_assertions": assertions,
            "validate_assertions": true,
            "index_for_attribution": false,
            "store_c2pa_manifest": false,
            "use_rights_profile": false,
            "fragment_pointers": false,
            "disable_c2pa": false
        }
    }))
}

/// Invoke the provider only after the job's explicit disclosure check.
pub(super) fn invoke(input: &Input) -> (Invocation, Option<Label>) {
    let started_at = Utc::now();
    let mut detail = json!({
        "manifest_definition": definition(input),
        "signing": {"basis": "Encypher service claim signer", "endpoint": ENDPOINT},
        "remote": {"request_attempted": false, "cost": null, "retention": "not established",
            "disclosure": input.policy.encypher},
    });
    if let SigningIdentity::Unusable { reason, .. } = input.signing {
        detail["signing"]["configuration_error"] = json!(reason);
    }
    let result = exchange(input, &mut detail);
    let (decision, method, outputs, mut gaps, label) = match result {
        Ok(label) => {
            let outputs = vec![
                ArtefactRef {
                    reference: input.labelled_reference.to_owned(),
                    content_hash: Some(sha256_digest(label.text.as_bytes())),
                    tokens: None,
                },
                ArtefactRef {
                    reference: input.manifest_reference.to_owned(),
                    content_hash: Some(sha256_digest(&label.manifest)),
                    tokens: None,
                },
            ];
            (
                Decision::Admit,
                "Encypher credential independently verified against the answer and source record",
                outputs,
                Vec::new(),
                Some(label),
            )
        }
        Err(reason) => (
            Decision::Abstain,
            "Encypher provenance unavailable; the answer stands unlabelled",
            Vec::new(),
            vec![Gap::new(GapReason::CapabilityUnavailable, reason)],
            None,
        ),
    };
    if detail["remote"]["request_attempted"] == true {
        gaps.push(Gap::new(
            GapReason::EvidenceMissing,
            "The Encypher signing charge is unreported; processor cost remains unknown.",
        ));
    }
    let mut inputs = vec![ArtefactRef {
        reference: input.answer_reference.to_owned(),
        content_hash: Some(sha256_digest(input.answer.as_bytes())),
        tokens: None,
    }];
    inputs.extend(input.sources.iter().map(|source| ArtefactRef {
        reference: source.reference.clone(),
        content_hash: Some(source.content_hash.clone()),
        tokens: None,
    }));
    let invocation = Invocation::new(
        manifest(),
        started_at,
        decision,
        method,
        inputs,
        outputs,
        detail,
        gaps,
        vec![
            "A trusted credential does not establish answer accuracy or source rights.",
            "The service certificate identifies the claim signer; operator identity is not independently established.",
            "Provider cost and complete retention are unknown; no online revocation check is made.",
            "Local fixtures do not establish live Encypher interoperability.",
        ],
    );
    (invocation, label)
}

fn exchange(input: &Input, detail: &mut Value) -> Result<Label, &'static str> {
    if !input
        .policy
        .encypher
        .as_ref()
        .is_some_and(|d| d.send_answer_and_source_record)
    {
        return Err("the suite has not authorised answer and source-record disclosure to Encypher");
    }
    let config = match input.signing {
        SigningIdentity::Encypher(config) => config,
        SigningIdentity::Unusable { .. } => {
            return Err(
                "Encypher signing configuration is unusable; check the selected signer, API key and trust bundle",
            );
        }
        _ => {
            return Err(
                "select COMMONMEASURE_PROVENANCE_SIGNER=encypher and configure its API key and trust bundle",
            );
        }
    };
    if input.answer.is_empty() || input.answer.len() > 1_000_000 {
        return Err("Encypher signing accepts a non-empty answer of at most 1 MB");
    }
    validate_trust_bundle(&config.trust_anchors)
        .map_err(|_| "the signing trust bundle is malformed")?;
    let body = request_body(input)?;
    let bytes = serde_json::to_vec(&body).map_err(|_| "cannot encode signing request")?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("Encypher signing request exceeds the adapter's 2 MiB limit");
    }
    let request_hash = sha256_digest(&bytes);
    detail["remote"]["request_hash"] = json!(request_hash);
    detail["remote"]["document_id"] = body["document_id"].clone();
    detail["remote"]["trust_anchors_hash"] = json!(sha256_digest(config.trust_anchors.as_bytes()));
    #[cfg(test)]
    if let Some(directory) = &config.capture_directory {
        std::fs::write(directory.join("request.json"), &bytes)
            .map_err(|_| "cannot save synthetic trial request")?;
    }
    let mut request = Request::post("/", bytes, "application/json");
    request
        .headers
        .set("Authorization", &format!("Bearer {}", config.api_key));
    request.headers.set("Idempotency-Key", &request_hash);
    detail["remote"]["request_attempted"] = json!(true);
    let response = commonmeasure_http::send(config.endpoint(), request)
        .map_err(|_| "Encypher transport failed; the service may have received the request; no retry was made")?;
    detail["remote"]["http_status"] = json!(response.status);
    detail["remote"]["response_hash"] = json!(sha256_digest(response.served_body()));
    #[cfg(test)]
    let capture = (response.body.clone(), response.served_body().to_vec());
    if response.status != 201 {
        return Err("Encypher did not return HTTP 201; no redirect or retry was attempted");
    }
    let response: Value =
        serde_json::from_slice(&response.body).map_err(|_| "Encypher returned malformed JSON")?;
    if response["success"] != true {
        return Err("Encypher did not report a successful signing operation");
    }
    let document = &response["data"]["document"];
    if document["document_id"] != body["document_id"] {
        return Err("Encypher returned a different document identifier");
    }
    if !document["coerced_options"].is_null()
        && document["coerced_options"]
            .as_object()
            .is_none_or(|v| !v.is_empty())
    {
        return Err("Encypher changed the requested signing options");
    }
    let text = document["signed_text"]
        .as_str()
        .ok_or("Encypher returned no signed text")?;
    let read = read_back_with_trust(text, Some(&config.trust_anchors))
        .map_err(|_| "Encypher returned an unreadable C2PA text credential")?;
    let mut validation = read.validation.clone();
    // URLs and explanations can contain service-chosen manifest labels. Keep
    // diagnostic codes, while raw successful evidence lives in the artefact.
    validation["statuses"] = json!(
        read.validation["statuses"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|status| json!({"code": status["code"]}))
            .collect::<Vec<_>>()
    );
    detail["validation"] = validation;
    if read.validation["state"] != "trusted" {
        return Err("Encypher credential is invalid or untrusted under the supplied anchors");
    }
    let normalised: String = input.answer.nfc().collect();
    if read.clean_text_hash != sha256_digest(normalised.as_bytes()) {
        return Err("Encypher changed the answer text");
    }
    let active = &read.manifest["manifests"][&read.manifest_label];
    for expected in [SOURCE_RECORD_LABEL, TRAINING_MINING_LABEL, "c2pa.actions"] {
        let instance_prefix = format!("{expected}__");
        let count = active["assertions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|a| {
                a["label"].as_str().is_some_and(|label| {
                    label == expected
                        || label.starts_with(&instance_prefix)
                        || (expected == "c2pa.actions" && label.starts_with("c2pa.actions.v"))
                })
            })
            .count();
        if count != 1 {
            return Err("Encypher returned missing or ambiguous required assertions");
        }
    }
    if read.source_record != source_record(input)
        || read.training_mining != input.policy.training_mining.assertion()
    {
        return Err("Encypher did not preserve the source record or training declaration");
    }
    let ingredients = &active["ingredients"];
    if read.ingredients != input.sources
        || ingredients.as_array().map_or(0, Vec::len) != input.sources.len()
        || ingredients
            .as_array()
            .into_iter()
            .flatten()
            .any(|i| i["relationship"] != "inputTo")
    {
        return Err("Encypher did not preserve the source ingredients and acquisition grades");
    }
    if !read.actions["actions"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|a| a["action"] == "c2pa.created" && a["digitalSourceType"] == DIGITAL_SOURCE_TYPE)
    {
        return Err("Encypher did not preserve the declared AI-generation action");
    }
    let manifest = c2pa_text::extract_manifest(text)
        .map_err(|_| "cannot extract returned manifest")?
        .manifest
        .ok_or("returned text has no manifest")?;
    if manifest
        .windows(config.api_key.len())
        .any(|part| part == config.api_key.as_bytes())
    {
        return Err("the returned credential contains the signing API key");
    }
    // Only the synthetic trial captures fully checked responses. Decoding JSON
    // before checking also catches escaped keys in service metadata.
    #[cfg(test)]
    if let Some(directory) = &config.capture_directory {
        if json_contains_key(&response, &config.api_key) {
            return Err("the synthetic trial response contains the signing API key");
        }
        std::fs::write(directory.join("response.json"), capture.0)
            .and_then(|_| std::fs::write(directory.join("response-served.bin"), capture.1))
            .map_err(|_| "cannot save synthetic trial response")?;
    }
    detail["manifest_label_hash"] = json!(sha256_digest(read.manifest_label.as_bytes()));
    detail["manifest_bytes"] = json!(manifest.len());
    detail["embedding"] = json!({"method": EMBEDDING, "normalisation": "NFC"});
    // Keep service-supplied certificate details in the raw credential artefact;
    // invocation diagnostics do not copy arbitrary response strings or bodies.
    Ok(Label {
        text: text.to_owned(),
        manifest,
    })
}

#[cfg(test)]
fn json_contains_key(value: &Value, key: &str) -> bool {
    match value {
        Value::String(value) => value.contains(key),
        Value::Array(values) => values.iter().any(|value| json_contains_key(value, key)),
        Value::Object(values) => values
            .iter()
            .any(|(name, value)| name.contains(key) || json_contains_key(value, key)),
        _ => false,
    }
}
