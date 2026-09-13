//! The loopback origin every adapter test points its `base_url` at.
//!
//! An origin binds a real `commonmeasure_http` server on `127.0.0.1:0`, records every
//! request the adapter puts on the wire, and answers with whatever body the
//! test supplies — a documented-shape response, or a recorded capture from
//! `demo/recon/`. Nothing else in the path is a stand-in: the client, the
//! framing and the parser under test are the ones a live call uses, which is
//! the substitution `AGENTS.md` permits.
//!
//! Each `tests/*.rs` is its own test crate and declares `mod common;`, so a
//! suite that needs only part of this module leaves the rest unused.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use commonmeasure_http::{Request, Response, ServerHandle};
use commonmeasure_types::canonical::{canonical_json, sha256_digest};
use serde_json::Value;

/// One request the adapter put on the wire.
#[derive(Debug, Clone)]
pub struct SeenRequest {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl SeenRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(seen, _)| seen.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn json_body(&self) -> Value {
        serde_json::from_slice(&self.body).expect("request body should be JSON")
    }
}

pub struct Origin {
    handle: ServerHandle,
    seen: Arc<Mutex<Vec<SeenRequest>>>,
}

impl Origin {
    /// Serve one recorded capture from `demo/recon/`: the recorded status, and
    /// the canonical JSON of the recorded body, which is exactly what the
    /// runtime's `--replay` origin puts on the wire and exactly what the
    /// declared hash covers (`docs/contracts/canonical-json.md`).
    ///
    /// [`read_capture`] has already refused a capture no manifest declares or
    /// whose bytes have moved, so this helper and the product path serve the
    /// same bytes and refuse the same alteration.
    pub fn replaying(capture: &str) -> Self {
        let recorded = read_capture(capture);
        let status = recorded["response"]["status"]
            .as_u64()
            .expect("capture records a status") as u16;
        let body = canonical_json(&recorded["response"]["body"]).into_bytes();
        Self::serving(move |_| {
            let mut response = Response::new(status, body.clone());
            response.headers.set("Content-Type", "application/json");
            response
        })
    }

    /// Serve whatever `handler` answers, recording each request first.
    pub fn serving<H>(handler: H) -> Self
    where
        H: Fn(&Request) -> Response + Send + Sync + 'static,
    {
        let seen: Arc<Mutex<Vec<SeenRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        let server =
            commonmeasure_http::Server::bind("127.0.0.1:0").expect("bind a loopback origin");
        let handle = server
            .spawn(move |request| {
                recorder.lock().expect("seen lock").push(SeenRequest {
                    method: request.method.clone(),
                    target: request.target.clone(),
                    headers: request
                        .headers
                        .iter()
                        .map(|(name, value)| (name.to_owned(), value.to_owned()))
                        .collect(),
                    body: request.body.clone(),
                });
                handler(&request)
            })
            .expect("spawn a loopback origin");
        Self { handle, seen }
    }

    pub fn url(&self) -> String {
        self.handle.url()
    }

    /// The one request a single-call adapter operation sends; a second request
    /// is a failed assertion, not a second element.
    pub fn only_request(&self) -> SeenRequest {
        let seen = self.seen.lock().expect("seen lock");
        assert_eq!(seen.len(), 1, "the adapter should send exactly one request");
        seen[0].clone()
    }

    /// Every request seen so far, in wire order, for a multi-leg exchange.
    pub fn requests(&self) -> Vec<SeenRequest> {
        self.seen.lock().expect("seen lock").clone()
    }
}

/// The recon directory.
fn recon() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/commonmeasure-supply sits two levels below the repository root")
        .join("demo/recon")
}

/// The declared hash of every committed capture a test may serve, by path
/// beneath `demo/recon/`.
///
/// Two manifests hold them. `replay-manifest.json` names the captures a
/// `--replay` run serves; `fixture-manifest.json` names the rest, the ones
/// that exercise a parser here and no run serves. A capture in neither is not
/// declared evidence and is refused below.
fn declared_hashes() -> BTreeMap<String, String> {
    manifest_field("response_sha256")
}

/// The endpoint each manifest declares for a capture, by path beneath
/// `demo/recon/`.
fn declared_endpoints() -> BTreeMap<String, String> {
    manifest_field("recorded_endpoint")
}

/// One declared field of every recording in both manifests, keyed by the
/// recording's source path.
fn manifest_field(field: &str) -> BTreeMap<String, String> {
    let mut declared = BTreeMap::new();
    for manifest in ["replay-manifest.json", "fixture-manifest.json"] {
        let path = recon().join(manifest);
        let bytes =
            std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let value: Value = serde_json::from_slice(&bytes).expect("a manifest is valid JSON");
        for recording in value["recordings"]
            .as_array()
            .unwrap_or_else(|| panic!("{manifest} lists no recordings"))
        {
            let source = recording["source"]
                .as_str()
                .expect("a recording names its source");
            let held = recording[field]
                .as_str()
                .unwrap_or_else(|| panic!("{source} declares no {field}"));
            declared.insert(source.to_owned(), held.to_owned());
        }
    }
    declared
}

/// The bytes a recording's declared hash covers: the canonical JSON of a
/// captured HTTP response's body, or the recorded body string of a captured
/// JSON-RPC leg. The same rule the runtime applies
/// (`crates/commonmeasure-runtime/src/replay.rs`), because a capture that one
/// path refuses and the other serves is not verified evidence.
fn recorded_body(capture: &Value) -> Vec<u8> {
    match capture["response"].is_object() {
        true => canonical_json(&capture["response"]["body"]).into_bytes(),
        false => capture["body"]
            .as_str()
            .expect("a captured leg records its body as a string")
            .as_bytes()
            .to_vec(),
    }
}

/// A committed recon capture, parsed and verified against the hash a manifest
/// declares for it; `relative` is the path beneath `demo/recon/`.
///
/// An altered capture is refused with both hashes, and a capture no manifest
/// declares is refused by name: a test must not serve bytes nothing vouches
/// for.
pub fn read_capture(relative: &str) -> Value {
    let path = recon().join(relative);
    let bytes =
        std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let capture: Value =
        serde_json::from_slice(&bytes).expect("a recon capture should be valid JSON");
    if let Err(refusal) = verify_capture(relative, &capture) {
        panic!("{refusal}");
    }
    capture
}

/// Check `capture` against the hash a manifest declares for `relative`, the
/// way the runtime's replay path checks the capture it is about to serve.
///
/// Separate from [`read_capture`] so the refusal itself can be asserted on: a
/// test cannot edit a committed capture to prove the check bites.
pub fn verify_capture(relative: &str, capture: &Value) -> Result<(), String> {
    let Some(expected) = declared_hashes().get(relative).cloned() else {
        return Err(format!(
            "{relative} is in no manifest; add it to demo/recon/replay-manifest.json if a run serves it, or demo/recon/fixture-manifest.json if only a test does"
        ));
    };
    let actual = sha256_digest(&recorded_body(capture));
    if actual != expected {
        return Err(format!(
            "{relative} no longer matches the hash its manifest declares: declared {expected}, computed {actual}"
        ));
    }
    // The same endpoint check the runtime's replay path applies: the URL the
    // manifest declares must be the one the capture says it called. An
    // HTTP-shaped capture records it under `request.url`, a JSON-RPC leg
    // under `url`; a capture recording neither is served on the manifest's
    // word, which is the only claim there is.
    let declared = declared_endpoints()
        .get(relative)
        .cloned()
        .unwrap_or_default();
    match capture["request"]["url"]
        .as_str()
        .or_else(|| capture["url"].as_str())
    {
        Some(recorded) if recorded != declared => Err(format!(
            "{relative} answers {recorded} and its manifest declares {declared}"
        )),
        _ => Ok(()),
    }
}
