//! Verify credentials against the received asset before the transform removes them.

use std::io::Cursor;

use commonmeasure_types::canonical::sha256_digest;
use serde_json::{Value, json};

/// A verification observation and, for Annex A.8, the text after wrapper removal.
pub(super) struct Inspection {
    pub evidence: Value,
    pub clean_text: Option<String>,
}

impl Inspection {
    fn new(state: &str, method: Option<&str>, reason: &str) -> Self {
        Self {
            evidence: json!({
                "state": state, "method": method, "reason": reason,
                "credential_ref": null, "manifest_label": null,
                "verifier": "c2pa-rs", "verifier_version": c2pa::VERSION,
                "trust_list": "none configured", "validation_status": [],
            }),
            clean_text: None,
        }
    }
}

pub(super) fn inspect(body: &[u8], content_type: Option<&str>) -> Inspection {
    let media_type = content_type
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let html = super::extract::is_html(&media_type);
    if !html && !media_type.starts_with("text/") {
        return Inspection::new(
            "unavailable",
            None,
            "Embedded credential verification requires a supported text content type; this media type is unsupported or missing.",
        );
    }
    // Lossy decoding would change the asset the credential is bound to.
    let Ok(text) = std::str::from_utf8(body) else {
        return Inspection::new(
            "unavailable",
            None,
            "Embedded credential verification requires UTF-8; the received body is not UTF-8.",
        );
    };
    if html {
        return match c2pa_text::html::extract_html(text) {
            Ok(Some(extracted)) => match extracted.method {
                c2pa_text::html::HtmlMethod::Inline => match extracted.manifest {
                    Some(manifest) => verify(&manifest, body, "text/html", "annex-a7-script"),
                    None => Inspection::new(
                        "invalid",
                        Some("annex-a7-script"),
                        "The inline manifest is not valid base64.",
                    ),
                },
                c2pa_text::html::HtmlMethod::Reference => {
                    let mut result = Inspection::new(
                        "unavailable",
                        Some("annex-a7-link"),
                        "External manifest retrieval is unavailable; no linked manifest was fetched.",
                    );
                    result.evidence["credential_ref"] = json!(extracted.reference);
                    result
                }
            },
            Err(error) => Inspection::new("invalid", Some("annex-a7"), &error.to_string()),
            Ok(None) => Inspection::new(
                "absent",
                Some("annex-a7"),
                "No supported embedded credential was found.",
            ),
        };
    }
    match c2pa_text::extract_manifest(text) {
        Ok(extracted) => match extracted.manifest {
            Some(manifest) => {
                let mut result = verify(&manifest, body, "text/plain", "annex-a8");
                result.clean_text = Some(extracted.clean_text);
                result
            }
            // The wrapper reader ignores unknown versions and truncated wrappers.
            // Preserve that uncertainty instead of reporting a successful check.
            None if text.chars().zip(text.chars().skip(1)).any(|(a, b)| {
                a == '\u{feff}' && matches!(b, '\u{fe00}'..='\u{fe0f}' | '\u{e0100}'..='\u{e01ef}')
            }) =>
            {
                Inspection::new(
                    "unavailable",
                    Some("annex-a8"),
                    "The text wrapper is unsupported or incomplete; the Annex A.8 reader could not recover a manifest.",
                )
            }
            None => Inspection::new(
                "absent",
                Some("annex-a8"),
                "No supported embedded credential was found.",
            ),
        },
        Err(error) => Inspection::new("invalid", Some("annex-a8"), &error.to_string()),
    }
}

fn verify(manifest: &[u8], body: &[u8], format: &str, method: &str) -> Inspection {
    let mut result = Inspection::new(
        "invalid",
        Some(method),
        "The manifest could not be verified against the received content.",
    );
    result.evidence["credential_ref"] = json!(sha256_digest(manifest));
    // Explicit settings keep the verifier offline and independent of ambient
    // SDK configuration. No supplier content can trigger a second acquisition.
    let mut settings = c2pa::Settings::default();
    settings.verify.remote_manifest_fetch = false;
    settings.verify.ocsp_fetch = false;
    let context = match c2pa::Context::new().with_settings(settings) {
        Ok(context) => context,
        Err(error) => {
            result.evidence["state"] = json!("unavailable");
            result.evidence["reason"] = json!(format!(
                "C2PA verifier configuration is unavailable: {error}"
            ));
            return result;
        }
    };
    match c2pa::Reader::from_context(context).with_manifest_data_and_stream(
        manifest,
        format,
        Cursor::new(body),
    ) {
        Ok(reader) => {
            result.evidence["manifest_label"] = json!(reader.active_label());
            result.evidence["validation_status"] = json!(reader.validation_status()
                .into_iter().flatten().map(|status| json!({
                    "code": status.code(), "url": status.url(), "explanation": status.explanation(),
                })).collect::<Vec<_>>());
            if reader.active_manifest().is_some() {
                let (state, reason) = match reader.validation_state() {
                    c2pa::ValidationState::Invalid => (
                        "invalid",
                        "The manifest does not verify against the received content.",
                    ),
                    c2pa::ValidationState::Valid | c2pa::ValidationState::Trusted => (
                        "untrusted",
                        "The signature and content binding verify; no signer trust list is configured.",
                    ),
                };
                result.evidence["state"] = json!(state);
                result.evidence["reason"] = json!(reason);
            }
        }
        Err(error) => {
            if matches!(
                error,
                c2pa::Error::UnsupportedType | c2pa::Error::CoseSignatureAlgorithmNotSupported
            ) {
                result.evidence["state"] = json!("unavailable");
            }
            result.evidence["reason"] =
                json!(format!("C2PA verification could not complete: {error}"));
        }
    }
    result
}
