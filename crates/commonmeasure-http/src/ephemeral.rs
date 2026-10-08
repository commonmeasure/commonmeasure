//! In-memory protection for a one-shot credential during enrolment.
//!
//! A setup command can call several clients which persist peer refusals.
//! Redaction at the terminal is too late for those clients. Scope the
//! protection to one origin and command lifetime, before responses reach
//! any parser or writer. Ordinary clients have no active protection.

use std::sync::Mutex;

use anyhow::{Result, anyhow, bail};
use serde_json::Value;

use crate::Response;

static ACTIVE: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Keeps an ephemeral credential out of an origin's responses for the
/// lifetime of a setup command, including requests made on other threads.
/// Nothing is written to disk. Drop it when the command's steps finish.
/// Requests to other origins and clients without a guard are unchanged.
pub struct EphemeralCredentialGuard {
    origin: String,
}

impl EphemeralCredentialGuard {
    /// Protect the trimmed credential sent to `url`'s origin. An overlapping
    /// guard for the same origin is refused rather than replacing its secret.
    pub fn new(url: &str, credential: &str) -> Result<Self> {
        let origin = url::Url::parse(url)?.origin().ascii_serialization();
        let credential = credential.trim();
        if credential.is_empty() {
            bail!("the enrolment token is empty");
        }
        let mut active = ACTIVE.lock().expect("ephemeral credential lock");
        if active.iter().any(|(held, _)| held == &origin) {
            bail!("an ephemeral credential is already protected for this origin");
        }
        active.push((origin.clone(), credential.to_owned()));
        Ok(Self { origin })
    }
}

impl Drop for EphemeralCredentialGuard {
    fn drop(&mut self) {
        ACTIVE
            .lock()
            .expect("ephemeral credential lock")
            .retain(|(origin, _)| origin != &self.origin);
    }
}

pub(crate) fn credential_for(url: &str) -> Option<String> {
    let origin = url::Url::parse(url).ok()?.origin().ascii_serialization();
    ACTIVE
        .lock()
        .expect("ephemeral credential lock")
        .iter()
        .find(|(held, _)| held == &origin)
        .map(|(_, credential)| credential.clone())
}

fn redact_json(value: &mut Value, credential: &str) {
    match value {
        Value::String(text) => *text = text.replace(credential, "<token>"),
        Value::Array(values) => {
            for value in values {
                redact_json(value, credential);
            }
        }
        Value::Object(members) => {
            *members = std::mem::take(members)
                .into_iter()
                .map(|(key, mut value)| {
                    redact_json(&mut value, credential);
                    (key.replace(credential, "<token>"), value)
                })
                .collect();
        }
        _ => {}
    }
}

pub(crate) fn protect(result: Result<Response>, credential: Option<&str>) -> Result<Response> {
    let Some(credential) = credential else {
        return result;
    };
    let mut response = result.map_err(|error| {
        let text = format!("{error:#}");
        if text.contains(credential) {
            anyhow!(text.replace(credential, "<token>"))
        } else {
            error
        }
    })?;
    // Decode JSON before searching: a peer can escape characters in the
    // token, and a later parser would restore them before persisting it.
    let mut json = serde_json::from_slice::<Value>(&response.body).ok();
    let json_echo = json.as_mut().is_some_and(|value| {
        let original = value.clone();
        redact_json(value, credential);
        *value != original
    });
    let echoed = String::from_utf8_lossy(&response.body).contains(credential) || json_echo;
    let header_echo = response.reason.contains(credential)
        || response
            .headers
            .iter()
            .any(|(name, value)| name.contains(credential) || value.contains(credential));
    if !echoed && !header_echo {
        return Ok(response);
    }
    // Never rewrite a successful response: it can carry a signed policy,
    // identity or credential. Refuse it before it can be activated or saved.
    if (200..300).contains(&response.status) {
        bail!(
            "the origin echoed the ephemeral enrolment token in a successful response; nothing from that response was accepted"
        );
    }
    response.reason = response.reason.replace(credential, "<token>");
    let mut headers = crate::Headers::new();
    for (name, value) in response.headers.iter() {
        headers.append(
            &name.replace(credential, "<token>"),
            &value.replace(credential, "<token>"),
        );
    }
    response.headers = headers;
    response.body = if let Some(value) = &mut json {
        serde_json::to_vec(value)?
    } else {
        String::from_utf8_lossy(&response.body)
            .replace(credential, "<token>")
            .into_bytes()
    };
    // Do not retain the unredacted compressed representation alongside the
    // redacted refusal. Callers receive only sanitised error evidence.
    response.coded = None;
    response.headers.remove("Content-Encoding");
    response.headers.remove("Transfer-Encoding");
    response
        .headers
        .set("Content-Length", &response.body.len().to_string());
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protection_is_origin_scoped_and_ends_with_the_guard() {
        let url = "http://127.0.0.1:18234/base";
        let guard = EphemeralCredentialGuard::new(url, "  et_unit  ").unwrap();
        assert_eq!(
            credential_for("http://127.0.0.1:18234/status").as_deref(),
            Some("et_unit")
        );
        assert!(credential_for("http://127.0.0.1:18235/status").is_none());
        assert!(EphemeralCredentialGuard::new(url, "other").is_err());
        drop(guard);
        assert!(credential_for(url).is_none());
    }

    #[test]
    fn compressed_refusals_and_error_chains_retain_no_secret() {
        let mut response = Response::text(401, "bad et_unit");
        response.reason = "et_unit".to_owned();
        response.headers.set("X-Detail", "et_unit");
        response.headers.set("Content-Encoding", "gzip");
        response.coded = Some(crate::CodedBody {
            coding: "gzip",
            bytes: b"et_unit".to_vec(),
        });
        let response = protect(Ok(response), Some("et_unit")).unwrap();
        assert_eq!(response.reason, "<token>");
        assert_eq!(response.headers.get("X-Detail"), Some("<token>"));
        assert!(response.headers.get("Content-Encoding").is_none());
        assert!(response.coded.is_none());
        assert_eq!(response.served_body(), b"bad <token>");
        let error = protect(
            Err(anyhow!("bad et_unit").context("peer et_unit")),
            Some("et_unit"),
        )
        .unwrap_err();
        assert_eq!(format!("{error:#}"), "peer <token>: bad <token>");
    }

    #[test]
    fn refusals_are_redacted_but_successes_are_not_rewritten() {
        let body = br#"{"detail":"bad \u0065t_unit", "et_unit": ["et_unit"]}"#;
        let refused = protect(
            Ok(Response::json(401, std::str::from_utf8(body).unwrap())),
            Some("et_unit"),
        )
        .unwrap();
        assert_eq!(refused.status, 401);
        assert!(!String::from_utf8_lossy(&refused.body).contains("et_unit"));
        assert!(String::from_utf8_lossy(&refused.body).contains("<token>"));
        assert!(protect(Ok(Response::new(200, body.to_vec())), Some("et_unit")).is_err());
        let plain = Response::new(401, body.to_vec());
        let unchanged = protect(Ok(plain.clone()), None).unwrap();
        assert_eq!(unchanged.status, plain.status);
        assert_eq!(unchanged.body, plain.body);
        assert_eq!(
            unchanged.headers.iter().collect::<Vec<_>>(),
            plain.headers.iter().collect::<Vec<_>>()
        );
    }
}
