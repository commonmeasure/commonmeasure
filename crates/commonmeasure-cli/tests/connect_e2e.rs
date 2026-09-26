//! Enrolment driven as an operator does it: the real binary connecting to a
//! hub, recording sessions, relaying, learning a revocation and
//! disconnecting. The offline tests use a loopback `commonmeasure_http::Server`
//! that answers the hub's enrolment routes and verifies the proof of
//! possession the binary sends, so the bytes on the wire are the ones a
//! real hub receives. Its answers copy the shapes of the hub's enrolment
//! handlers (exchange, status, disconnect, the directory-proof upload), of
//! its key directory and of its telemetry acceptance body; a change to any
//! of those shapes is a change here. It checks an uploaded directory proof
//! by the per-key rule in `webbotauth`, not by the edge's signing code. The
//! one test against a running hub is ignored by default; see
//! `a_real_hub_enrols_revokes_and_disconnects_this_edge` for how to run it.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use commonmeasure_harness::enrolment::RELEASE;
use commonmeasure_http::{Request, Response, Server, ServerHandle, send};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

mod webbotauth;

const TOKEN: &str = "et_0123456789abcdef";
const API_KEY: &str = "ak_hub_issued_secret";
/// The origin this hub says it publishes the enrolled key under. The edge
/// stores it and names it in `Signature-Agent` on every signed request; it
/// never invents an origin of its own.
const ORIGIN: &str = "https://hub.example";
/// The raw Ed25519 public key this hub says it signs policy with, hex.
const SIGNER_PUBLIC_KEY: &str = "3d6b4ad857f44f933254d7b5b4199800db753f9ec8183f4e0a0772e0031943b7";

/// What the loopback hub saw and how it answers.
#[derive(Default)]
struct HubState {
    /// Every request received, on any route.
    requests: usize,
    /// The exchange request body, verbatim.
    exchange_bodies: Vec<Value>,
    /// The key id the hub assigned (the thumbprint of the registered key).
    key_id: Option<String>,
    /// Batches delivered to the telemetry receiver.
    batches: Vec<Value>,
    /// `X-API-Key` values seen on status, disconnect and signer calls.
    status_keys: Vec<Option<String>>,
    disconnect_keys: Vec<Option<String>>,
    signer_keys: Vec<Option<String>>,
    /// When set, the status route answers a revocation.
    revoked: Option<(&'static str, &'static str)>,
    /// When set, every route taking the ingest key refuses it with 401,
    /// which is what a key revoked at the hub or a closed organisation
    /// answers.
    refuse_key: bool,
    status_error: Option<(u16, &'static str)>,
    /// A raw status answer, as an ingress or proxy in front of the hub
    /// writes one, with no media type unless one is given.
    status_raw: Option<(u16, String, Option<&'static str>)>,
    /// When set, the signer route answers 401 to the edge, as a hub whose
    /// signer route takes an owner's session and not an ingest key does.
    refuse_signer: bool,
    /// When set, the signer route also carries the absolute `policy_url`
    /// the hub builds from its public origin, which the edge must prefer to
    /// the address it typed.
    signer_policy_url: Option<&'static str>,
    policy_response: Option<(u16, &'static str)>,
    /// When set, the exchange and status answers carry the hub's
    /// `directory_proof` statement with this authority, and the hub takes
    /// uploads. Unset, the hub is one that takes no directory proofs.
    proof_authority: Option<&'static str>,
    /// The enrolled public key, base64url, as the exchange registered it.
    x: Option<String>,
    /// Upload request bodies, verbatim.
    uploads: Vec<Value>,
    /// When set, an upload is refused with this status and detail.
    refuse_upload: Option<(u16, &'static str)>,
    /// A raw error response, before proof acceptance.
    upload_error: Option<(u16, String)>,
    /// An answer substituted after proof acceptance, for malformed replies.
    upload_answer: Option<String>,
    /// The listing decision on exchange, status and upload answers.
    listing_decision: Option<(bool, Option<&'static str>)>,
    /// A changed decision first learnt on the next upload.
    upload_decision: Option<(bool, Option<&'static str>)>,
    /// The proof the hub holds: the params, the signature and the expiry.
    held: Option<(String, String, i64)>,
    /// When set, the exchange answers this `telemetry_path` in place of
    /// `/api/v1/telemetry`.
    telemetry_path: Option<&'static str>,
}

/// The lifetime this hub states and accepts, seconds.
const LIFETIME_SECS: i64 = 604_800;

fn rfc3339(seconds: i64) -> String {
    chrono::DateTime::from_timestamp(seconds, 0)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

impl HubState {
    /// The `directory_proof` member as the hub states it, or nothing.
    fn statement(&self) -> Option<Value> {
        self.proof_authority.map(|authority| {
            let mut statement = json!({
                "lifetime_secs": LIFETIME_SECS,
                "authority": authority,
                "expires_at": self.held.as_ref().map(|(_, _, expires)| rfc3339(*expires)),
            });
            if let Some((listed, reason)) = self.listing_decision {
                statement["listed"] = json!(listed);
                statement["unlisted_reason"] = json!(reason);
            }
            statement
        })
    }

    /// The key directory this hub serves: the key while it stands and holds
    /// a proof at least two cache ages from expiry, with its member.
    fn directory(&self) -> webbotauth::Directory {
        let mut directory = webbotauth::Directory::default();
        if let (Some(x), Some((params, signature, expires)), None) =
            (&self.x, &self.held, self.revoked)
            && *expires - now() >= 7_200
            && self.listing_decision.is_none_or(|(listed, _)| listed)
        {
            let raw: [u8; 32] = URL_SAFE_NO_PAD.decode(x).unwrap().try_into().unwrap();
            directory.publish(&raw, params, signature);
        }
        directory
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Fetch the loopback hub's key directory and apply the per-key rule to it,
/// returning the key ids a verifier may use.
fn usable_in_directory(hub_url: &str) -> Vec<String> {
    let response = send(
        &format!("{hub_url}/.well-known/http-message-signatures-directory"),
        Request::get("/"),
    )
    .expect("the hub answers");
    assert_eq!(response.status, 200);
    let usable = webbotauth::usable_keys(
        &String::from_utf8_lossy(&response.body),
        response.headers.get("Signature-Input"),
        response.headers.get("Signature"),
        "hub.example",
        now(),
    )
    .expect("a directory a verifier can read");
    usable.keys.into_keys().collect()
}

fn thumbprint(x: &str) -> String {
    let canonical = format!(r#"{{"crv":"Ed25519","kty":"OKP","x":"{x}"}}"#);
    URL_SAFE_NO_PAD.encode(Sha256::digest(canonical.as_bytes()))
}

fn hub(state: Arc<Mutex<HubState>>) -> ServerHandle {
    Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(move |request: Request| {
            let mut state = state.lock().unwrap();
            state.requests += 1;
            let api_key = request.headers.get("X-API-Key").map(str::to_owned);
            match (request.method.as_str(), request.target.as_str()) {
                ("POST", "/api/v1/enrolment/exchange") => {
                    let body: Value = serde_json::from_slice(&request.body).unwrap();
                    state.exchange_bodies.push(body.clone());
                    if body["token"] != json!(TOKEN) {
                        return Response::json(401, r#"{"detail":"enrolment token is unknown"}"#);
                    }
                    let x = body["public_key"]["x"].as_str().unwrap();
                    let raw: [u8; 32] = URL_SAFE_NO_PAD.decode(x).unwrap().try_into().unwrap();
                    let key = VerifyingKey::from_bytes(&raw).unwrap();
                    let proof: [u8; 64] = URL_SAFE_NO_PAD
                        .decode(body["proof"].as_str().unwrap())
                        .unwrap()
                        .try_into()
                        .unwrap();
                    if key
                        .verify(TOKEN.as_bytes(), &Signature::from_bytes(&proof))
                        .is_err()
                    {
                        return Response::json(400, r#"{"detail":"proof does not verify"}"#);
                    }
                    let key_id = thumbprint(x);
                    state.key_id = Some(key_id.clone());
                    state.x = Some(x.to_owned());
                    let mut answer = json!({
                        "organization": {"id": "11111111-1111-1111-1111-111111111111", "name": "Org A Media"},
                        "name": "laptop-7",
                        "key_id": key_id,
                        "identity": {
                            "origin": ORIGIN,
                            "bot_page": "https://hub.example/bot",
                            "contact": "mailto:bot@hub.example",
                        },
                        "api_key_id": "22222222-2222-2222-2222-222222222222",
                        "api_key": API_KEY,
                        "telemetry_path": state.telemetry_path.unwrap_or("/api/v1/telemetry"),
                    });
                    if let Some(statement) = state.statement() {
                        answer["directory_proof"] = statement;
                    }
                    Response::json(201, &answer.to_string())
                }
                // The upload, checked by the per-key rule a verifier applies
                // to the directory it will be served in, not by the edge's
                // own signing code.
                ("PUT", "/api/v1/enrolment/directory-proof") => {
                    let body: Value = serde_json::from_slice(&request.body).unwrap();
                    state.uploads.push(body.clone());
                    if api_key.as_deref() != Some(API_KEY) {
                        return Response::json(404, r#"{"detail":"no edge key for this ingest key"}"#);
                    }
                    if let Some((status, body)) = &state.upload_error {
                        return Response::new(*status, body.as_bytes().to_vec());
                    }
                    if let Some((status, detail)) = state.refuse_upload {
                        let body = json!({"detail": detail});
                        return Response::json(status, &body.to_string());
                    }
                    if body["key_id"] != json!(state.key_id) {
                        return Response::json(422, r#"{"detail":"keyid is not this edge's key"}"#);
                    }
                    let params = body["signature_input"].as_str().unwrap().to_owned();
                    let signature = body["signature"].as_str().unwrap().to_owned();
                    let raw: [u8; 32] = URL_SAFE_NO_PAD
                        .decode(state.x.as_deref().unwrap())
                        .unwrap()
                        .try_into()
                        .unwrap();
                    let mut candidate = webbotauth::Directory::default();
                    candidate.publish(&raw, &params, &signature);
                    let (directory, input, served) = candidate.response();
                    let usable = webbotauth::usable_keys(
                        &directory,
                        input.as_deref(),
                        served.as_deref(),
                        state.proof_authority.unwrap(),
                        now(),
                    )
                    .unwrap();
                    let key_id = state.key_id.clone().unwrap();
                    if let Some(reason) = usable.refused.get(&key_id) {
                        return Response::json(422, &json!({"detail": reason}).to_string());
                    }
                    let field = |name: &str| -> i64 {
                        let at = params.find(&format!(";{name}=")).unwrap() + name.len() + 2;
                        params[at..].split(';').next().unwrap().parse().unwrap()
                    };
                    let (created, expires) = (field("created"), field("expires"));
                    if expires - created > LIFETIME_SECS || expires - now() < 7_200 {
                        return Response::json(422, r#"{"detail":"the validity window is refused"}"#);
                    }
                    if state
                        .held
                        .as_ref()
                        .is_none_or(|(_, _, held)| expires > *held)
                    {
                        state.held = Some((params, signature, expires));
                    }
                    if let Some(decision) = state.upload_decision.take() {
                        state.listing_decision = Some(decision);
                    }
                    if let Some(body) = &state.upload_answer {
                        return Response::json(200, body);
                    }
                    Response::json(200, &json!({
                        "key_id": state.key_id,
                        "stored": true,
                        "directory_proof": state.statement(),
                    }).to_string())
                }
                ("GET", "/.well-known/http-message-signatures-directory") => {
                    let (body, input, signature) = state.directory().response();
                    let mut response = Response::json(200, &body);
                    if let (Some(input), Some(signature)) = (input, signature) {
                        response.headers.set("Signature-Input", &input);
                        response.headers.set("Signature", &signature);
                    }
                    response
                }
                ("GET", "/api/v1/enrolment/status") => {
                    state.status_keys.push(api_key.clone());
                    if let Some((status, body, media)) = &state.status_raw {
                        let mut response = Response::new(*status, body.as_bytes().to_vec());
                        if let Some(media) = media {
                            response.headers.set("Content-Type", media);
                        }
                        return response;
                    }
                    if let Some((status, detail)) = state.status_error {
                        return Response::json(status, &json!({"detail": detail}).to_string());
                    }
                    if api_key.as_deref() != Some(API_KEY) || state.refuse_key {
                        return Response::json(401, r#"{"detail":"a valid credential is required"}"#);
                    }
                    let (revoked_at, revocation) = match state.revoked {
                        Some((at, by)) => (json!(at), json!(by)),
                        None => (Value::Null, Value::Null),
                    };
                    let mut answer = json!({
                        "key_id": state.key_id,
                        "name": "laptop-7",
                        "revoked_at": revoked_at,
                        "revocation": revocation,
                    });
                    if let Some(statement) = state.statement() {
                        answer["directory_proof"] = statement;
                    }
                    Response::json(200, &answer.to_string())
                }
                ("POST", "/api/v1/enrolment/disconnect") => {
                    // Match the hosted load balancer's empty-POST requirement.
                    if request.headers.get("Content-Length") != Some("0") {
                        return Response::new(411, Vec::new());
                    }
                    state.disconnect_keys.push(api_key.clone());
                    if api_key.as_deref() != Some(API_KEY) {
                        return Response::json(401, r#"{"detail":"a valid credential is required"}"#);
                    }
                    Response::new(204, Vec::new())
                }
                // The policy signer an edge pins, read under the ingest key
                // (`docs/contracts/policy-envelope.md` §The hub side).
                ("GET", "/api/v1/policy/signer") => {
                    state.signer_keys.push(api_key.clone());
                    if api_key.as_deref() != Some(API_KEY) || state.refuse_signer {
                        return Response::json(401, r#"{"detail":"a valid credential is required"}"#);
                    }
                    let mut signer = json!({
                        "algorithm": "ed25519",
                        "key_id": "hub-policy-test",
                        "organisation": "11111111-1111-1111-1111-111111111111",
                        "policy_path": "/api/v1/policy/desired",
                        "public_key": SIGNER_PUBLIC_KEY,
                    });
                    if let Some(url) = state.signer_policy_url {
                        signer["policy_url"] = json!(url);
                    }
                    Response::json(200, &signer.to_string())
                }
                ("GET", "/api/v1/policy/desired") => {
                    let (status, body) = state.policy_response.unwrap_or((404,
                        r#"{"detail":"no policy revision has been published for this organisation"}"#));
                    Response::json(status, body)
                }
                ("POST", "/api/v1/telemetry/events") => {
                    if api_key.as_deref() != Some(API_KEY) || state.refuse_key {
                        return Response::json(401, r#"{"detail":"a valid credential is required"}"#);
                    }
                    let body: Value = serde_json::from_slice(&request.body).unwrap();
                    let events = body["events"].as_array().map(Vec::len).unwrap_or(0);
                    state.batches.push(body);
                    Response::json(
                        201,
                        &json!({"status": "ok", "events_created": events}).to_string(),
                    )
                }
                _ => Response::json(404, r#"{"detail":"not found"}"#),
            }
        })
        .unwrap()
}

fn commonmeasure(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(args)
        .env("COMMONMEASURE_HOME", home)
        .output()
        .expect("the binary should run")
}

fn session_start(home: &Path, session: &str) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["hook", "session-start"])
        .env("COMMONMEASURE_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("hook should start");
    writeln!(
        child.stdin.as_mut().unwrap(),
        "{}",
        json!({"session_id": session, "hook_event_name": "SessionStart", "source": "startup"})
    )
    .unwrap();
    assert!(child.wait().expect("wait").success());
}

fn record_cleared_crossing(home: &Path, session: &str, url: &str) {
    std::fs::write(
        home.join("policy.json"),
        r#"{"scopes":[{"match":"/work/personal","engagement":"personal","allow_telemetry_egress":true}]}"#,
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["hook", "post-tool-use"])
        .env("COMMONMEASURE_HOME", home)
        .stdin(Stdio::piped())
        .spawn()
        .expect("hook should start");
    writeln!(
        child.stdin.as_mut().unwrap(),
        "{}",
        json!({
            "session_id": session,
            "cwd": "/work/personal",
            "hook_event_name": "PostToolUse",
            "tool_name": "WebFetch",
            "tool_input": {"url": url},
            "tool_response": {"result": "Enough page text to count as grounded."}
        })
    )
    .unwrap();
    assert!(child.wait().expect("wait").success());
}

/// Start the mediated server for one session and let it exit on end of
/// input: what a host does at session start, before any tool call.
fn mcp_session(home: &Path, session: &str) {
    let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["mcp", "--host", "claude-code", "--session", session])
        .env("COMMONMEASURE_HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .expect("the server should run");
    assert!(
        output.status.success(),
        "the server should exit cleanly: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn session_records(home: &Path, session: &str) -> Vec<Value> {
    let text = std::fs::read_to_string(home.join("sessions").join(format!("{session}.ndjson")))
        .expect("the session log exists");
    text.lines()
        .map(|line| serde_json::from_str(line).expect("ndjson"))
        .collect()
}

fn edge_identity_records(home: &Path, session: &str) -> Vec<Value> {
    session_records(home, session)
        .into_iter()
        .filter(|record| record["event"] == json!("edge_identity"))
        .collect()
}

struct Console {
    child: Child,
    base: String,
}

impl Console {
    fn start(home: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
            .args(["serve", "--listen", "127.0.0.1:0"])
            .env("COMMONMEASURE_HOME", home)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the binary should start");
        let stdout = child.stdout.as_mut().expect("stdout");
        let mut lines = BufReader::new(stdout).lines();
        let base = loop {
            let line = lines
                .next()
                .expect("serve announces its address before serving")
                .expect("readable stdout");
            if let Some(address) = line.strip_prefix("listening on ") {
                break address.trim().to_owned();
            }
        };
        Self { child, base }
    }

    fn status(&self) -> Value {
        let response = send(&format!("{}/api/status", self.base), Request::get("/"))
            .expect("the request should complete");
        assert_eq!(response.status, 200);
        serde_json::from_slice(&response.body).expect("the API answers JSON")
    }

    /// The Overview as the operator reads it.
    fn overview(&self) -> String {
        let response = send(&format!("{}/", self.base), Request::get("/"))
            .expect("the request should complete");
        assert_eq!(response.status, 200);
        String::from_utf8_lossy(&response.body).into_owned()
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn connect_with_nothing_named_refuses_and_names_what_is_missing() {
    let home = tempfile::tempdir().unwrap();
    let output = commonmeasure(home.path(), &["connect"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no hub URL and no --token"), "{stderr}");
    assert!(stderr.contains("no default hub"), "{stderr}");
    assert!(!home.path().join("relay.json").exists());
    assert!(!home.path().join("edge-key.json").exists());

    let output = commonmeasure(home.path(), &["connect", "https://hub.example"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no --token"), "{stderr}");
    assert!(!home.path().join("edge-key.json").exists());
}

/// The whole life of an enrolment: connect, sessions naming the key id, a
/// relay run, a revocation learnt on the next run and named by the next
/// session, the console's egress block, and disconnect.
#[test]
fn an_edge_enrols_records_its_key_id_learns_revocation_and_disconnects() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(HubState::default()));
    let mut server = hub(state.clone());
    let hub_url = server.url();

    let output = commonmeasure(
        home.path(),
        &["connect", &format!("{hub_url}/"), "--token", TOKEN],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "connect failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // What went over the wire: the public key and a proof, never the
    // private key; the hub verified the proof before answering.
    let key_id = {
        let state = state.lock().unwrap();
        assert_eq!(state.exchange_bodies.len(), 1);
        let body = &state.exchange_bodies[0];
        assert_eq!(body["public_key"]["kty"], json!("OKP"));
        assert!(
            body["public_key"].get("d").is_none(),
            "the private key left the machine"
        );
        assert!(!body.to_string().contains(API_KEY));
        state.key_id.clone().expect("the hub assigned a key id")
    };

    // What was written: the private key, owner-only; the enrolment record;
    // the relay configuration carrying the ingest key.
    let private: Value =
        serde_json::from_slice(&std::fs::read(home.path().join("edge-key.json")).unwrap()).unwrap();
    assert!(private["d"].is_string());
    assert_eq!(thumbprint(private["x"].as_str().unwrap()), key_id);
    // Both files that hold a credential are owner-readable only.
    #[cfg(unix)]
    for file in ["edge-key.json", "relay.json"] {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(home.path().join(file))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{file} is readable by others");
    }
    let enrolment: Value =
        serde_json::from_slice(&std::fs::read(home.path().join("enrolment.json")).unwrap())
            .unwrap();
    assert_eq!(enrolment["key_id"], json!(key_id));
    assert_eq!(enrolment["hub"], json!(hub_url));
    assert_eq!(enrolment["organization"]["name"], json!("Org A Media"));
    assert_eq!(enrolment["identity"]["origin"], json!(ORIGIN));
    assert_eq!(
        enrolment["identity"]["bot_page"],
        json!("https://hub.example/bot")
    );
    assert!(enrolment.get("revoked_at").is_none());
    let relay_config: Value =
        serde_json::from_slice(&std::fs::read(home.path().join("relay.json")).unwrap()).unwrap();
    assert_eq!(
        relay_config["receiver"],
        json!(format!("{hub_url}/api/v1/telemetry"))
    );
    assert_eq!(relay_config["api_key"], json!(API_KEY));

    // What was said: the key id and the receiver, the first relay run's
    // account, and never the ingest key.
    assert!(stdout.contains(&key_id), "{stdout}");
    assert!(stdout.contains("Org A Media"), "{stdout}");
    assert!(stdout.contains("first relay run:"), "{stdout}");
    assert!(stdout.contains("enrolled"), "{stdout}");
    assert!(
        !stdout.contains(API_KEY),
        "the ingest key was printed: {stdout}"
    );
    assert_eq!(
        state.lock().unwrap().status_keys,
        vec![Some(API_KEY.to_owned())],
        "the first relay run proved the ingest key at the hub"
    );

    // Every session from now on names the key id, whichever path opens
    // the log: a host's session-start hook, or the mediated server.
    session_start(home.path(), "s-enrolled");
    let identity = edge_identity_records(home.path(), "s-enrolled");
    assert_eq!(identity.len(), 1);
    assert_eq!(identity[0]["payload"]["key_id"], json!(key_id));
    assert_eq!(identity[0]["payload"]["standing"], json!("enrolled"));
    assert_eq!(identity[0]["payload"]["hub"], json!(hub_url));
    // This hub takes no directory proofs: nothing was uploaded, and the
    // record says the directory carries no signature by this key.
    assert!(state.lock().unwrap().uploads.is_empty());
    assert!(identity[0]["payload"].get("listed_until").is_none());
    let unlisted = identity[0]["payload"]["unlisted"]
        .as_str()
        .expect("a reason");
    assert!(unlisted.contains("stated no directory proof"), "{unlisted}");
    mcp_session(home.path(), "s-mediated");
    let identity = edge_identity_records(home.path(), "s-mediated");
    assert_eq!(identity.len(), 1, "the mediated server start names the key");
    assert_eq!(identity[0]["payload"]["key_id"], json!(key_id));
    assert_eq!(identity[0]["payload"]["standing"], json!("enrolled"));

    // The fleet-status document names the key id as the edge's identity,
    // read from the enrolment record the exchange wrote.
    let output = commonmeasure(home.path(), &["status", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["edge"]["key_id"], json!(key_id));
    assert!(status["edge"].get("unknown").is_none(), "{status}");

    // A relay run delivers under the ingest key and states the standing.
    record_cleared_crossing(home.path(), "s-enrolled", "https://www.example.com/page");
    let output = commonmeasure(home.path(), &["relay"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains(&format!("edge key {key_id}: enrolled")),
        "{stdout}"
    );
    assert_eq!(state.lock().unwrap().batches.len(), 1);

    // The console's egress block names the receiver, the count and the
    // key, and the Overview renders them.
    {
        let console = Console::start(home.path());
        let status = console.status();
        assert_eq!(
            status["egress"]["receiver"],
            json!(format!("{hub_url}/api/v1/telemetry"))
        );
        assert_eq!(status["egress"]["delivered"], 2);
        assert_eq!(status["egress"]["key_id"], json!(key_id));
        assert_eq!(status["egress"]["key_standing"], json!("enrolled"));
        let overview = console.overview();
        assert!(overview.contains(&key_id), "the Overview names the key id");
        assert!(
            overview.contains("enrolled."),
            "the Overview states the standing"
        );
        assert!(overview.contains(&format!("{hub_url}/api/v1/telemetry")));
    }

    // The owner revokes the key at the hub. The edge learns on its next
    // relay run and every later session says so.
    state.lock().unwrap().revoked = Some(("2026-09-06T08:00:00Z", "owner"));
    let output = commonmeasure(home.path(), &["relay"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains(&format!(
            "edge key {key_id}: revoked at 2026-09-06T08:00:00Z by the owner"
        )),
        "{stdout}"
    );
    let enrolment: Value =
        serde_json::from_slice(&std::fs::read(home.path().join("enrolment.json")).unwrap())
            .unwrap();
    assert_eq!(enrolment["revoked_at"], json!("2026-09-06T08:00:00Z"));
    assert_eq!(enrolment["revocation"], json!("owner"));
    assert!(enrolment["revocation_learnt_at"].is_string());

    session_start(home.path(), "s-after-revocation");
    let identity = edge_identity_records(home.path(), "s-after-revocation");
    assert_eq!(identity[0]["payload"]["key_id"], json!(key_id));
    assert_eq!(identity[0]["payload"]["standing"], json!("revoked"));
    assert_eq!(
        identity[0]["payload"]["revoked_at"],
        json!("2026-09-06T08:00:00Z")
    );
    assert_eq!(identity[0]["payload"]["revocation"], json!("owner"));

    // Disconnect: the hub is told under the ingest key, and the files go.
    let output = commonmeasure(home.path(), &["disconnect"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains(&format!("revoked edge key {key_id} and its ingest key")),
        "{stdout}"
    );
    assert_eq!(
        state.lock().unwrap().disconnect_keys,
        vec![Some(API_KEY.to_owned())]
    );
    for file in ["relay.json", "edge-key.json", "enrolment.json"] {
        assert!(
            !home.path().join(file).exists(),
            "{file} survived disconnect"
        );
    }
    // The durable account of what was delivered stays.
    assert!(home.path().join("relay").join("delivered.idx").exists());

    // Not enrolled: a session names no key, the status document says the
    // edge is not enrolled, and the relay has no receiver.
    session_start(home.path(), "s-disconnected");
    assert!(edge_identity_records(home.path(), "s-disconnected").is_empty());
    let output = commonmeasure(home.path(), &["status", "--json"]);
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["edge"]["key_id"], Value::Null);
    assert!(
        status["edge"]["unknown"]
            .as_str()
            .is_some_and(|reason| reason.contains("not enrolled")),
        "{status}"
    );
    let output = commonmeasure(home.path(), &["relay"]);
    assert!(!output.status.success());
    let output = commonmeasure(home.path(), &["disconnect"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not enrolled"));

    server.stop();
}

fn directory_listing(home: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(home.join("directory-listing.json")).unwrap()).unwrap()
}

/// Set the expiry of the proof the listing file says the hub holds.
fn age_listing(home: &Path, expires: i64) {
    let mut listing = directory_listing(home);
    listing["stated"]["expires_at"] = json!(rfc3339(expires));
    std::fs::write(
        home.join("directory-listing.json"),
        serde_json::to_vec_pretty(&listing).unwrap(),
    )
    .unwrap();
}

/// The enrolment record exactly as the 0.3.1 release defines it, copied from
/// `crates/commonmeasure-harness/src/enrolment.rs` at the `v0.3.1` tag with
/// the documentation stripped. A released binary and a newer one share one
/// operator home during an upgrade, and the released binary refuses a record
/// with a member it does not know, so what this build writes to
/// `enrolment.json` must load through this type. Never edit it to follow the
/// current type.
mod released_0_3_1 {
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    pub struct EnrolledIdentity {
        pub origin: String,
        pub bot_page: String,
        #[serde(default)]
        pub contact: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    pub struct EnrolledOrganization {
        pub id: String,
        pub name: String,
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    pub struct EnrolmentRecord {
        pub hub: String,
        pub organization: EnrolledOrganization,
        pub name: String,
        pub key_id: String,
        pub identity: EnrolledIdentity,
        pub enrolled_at: String,
        #[serde(default)]
        pub revoked_at: Option<String>,
        #[serde(default)]
        pub revocation: Option<String>,
        #[serde(default)]
        pub revocation_learnt_at: Option<String>,
    }

    /// Load `<home>/enrolment.json` as the 0.3.1 release does.
    pub fn load(home: &std::path::Path) -> Result<EnrolmentRecord, String> {
        let bytes =
            std::fs::read(home.join("enrolment.json")).map_err(|error| error.to_string())?;
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())
    }
}

/// The directory proof across an edge's life against a hub that takes them:
/// signed and uploaded at `connect`, verifying under the per-key rule in the
/// directory the hub serves, left alone while it is current, renewed at a
/// session start once it is more than a day old, and gone with revocation.
///
/// Catches: an edge that enrols a key no verifier will use, one that signs
/// and uploads on every contact, and a session record that claims a listing
/// the directory does not carry.
#[test]
fn connect_lists_the_key_with_a_proof_that_verifies_and_a_start_renews_it_when_due() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(HubState {
        proof_authority: Some("hub.example"),
        ..HubState::default()
    }));
    let mut server = hub(state.clone());
    let hub_url = server.url();

    let output = commonmeasure(home.path(), &["connect", &hub_url, "--token", TOKEN]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("directory proof: signed and accepted by the hub"),
        "{stdout}"
    );
    let key_id = state.lock().unwrap().key_id.clone().unwrap();
    {
        let state = state.lock().unwrap();
        assert_eq!(state.uploads.len(), 1, "one proof, not one per request");
        let upload = &state.uploads[0];
        assert_eq!(upload["key_id"], json!(key_id));
        assert_eq!(upload["release"], RELEASE);
        assert!(
            upload["signature_input"]
                .as_str()
                .unwrap()
                .starts_with(r#"("@authority";req);"#),
            "{upload}"
        );
        let private: Value =
            serde_json::from_slice(&std::fs::read(home.path().join("edge-key.json")).unwrap())
                .unwrap();
        assert!(
            !upload.to_string().contains(private["d"].as_str().unwrap()),
            "the private key left the machine"
        );
    }
    assert_eq!(usable_in_directory(&hub_url), vec![key_id.clone()]);
    let listing = directory_listing(home.path());
    assert_eq!(listing["key_id"], json!(key_id));
    let expires_at = listing["stated"]["expires_at"]
        .as_str()
        .expect("the held proof's expiry is recorded")
        .to_owned();

    // A session start with a current proof asks the hub nothing, and names
    // the listing: the held proof's expiry less the hub's serving margin.
    let status_calls = state.lock().unwrap().status_keys.len();
    session_start(home.path(), "s-listed");
    assert_eq!(state.lock().unwrap().status_keys.len(), status_calls);
    let identity = &edge_identity_records(home.path(), "s-listed")[0]["payload"];
    let listed_until = identity["listed_until"].as_str().expect("listed");
    let expected = chrono::DateTime::parse_from_rfc3339(&expires_at).unwrap()
        - chrono::Duration::seconds(7_200);
    assert_eq!(
        chrono::DateTime::parse_from_rfc3339(listed_until).unwrap(),
        expected
    );
    assert!(identity.get("unlisted").is_none(), "{identity}");

    // A relay run reads the hub's statement and sends nothing.
    let output = commonmeasure(home.path(), &["relay"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("directory proof: current"), "{stdout}");
    assert_eq!(state.lock().unwrap().uploads.len(), 1);

    // Two days on, as far as the hub and the record are concerned: the
    // next session start asks and renews.
    let aged = now() + LIFETIME_SECS - 2 * 86_400;
    {
        let mut state = state.lock().unwrap();
        let held = state.held.as_mut().unwrap();
        held.2 = aged;
    }
    age_listing(home.path(), aged);
    session_start(home.path(), "s-renewed");
    assert_eq!(state.lock().unwrap().uploads.len(), 2, "renewed once");
    assert_eq!(state.lock().unwrap().uploads[1]["release"], RELEASE);
    let identity = &edge_identity_records(home.path(), "s-renewed")[0]["payload"];
    let renewed =
        chrono::DateTime::parse_from_rfc3339(identity["listed_until"].as_str().unwrap()).unwrap();
    assert!(
        renewed.timestamp() >= expected.timestamp() && renewed.timestamp() > aged - 7_200,
        "listed until the renewed proof's expiry, not the aged one's: {identity}"
    );
    assert_eq!(usable_in_directory(&hub_url), vec![key_id.clone()]);

    // The mediated server's start is the other path that opens a session,
    // and a host without a session-start hook reaches the hub only there.
    let aged = now() + LIFETIME_SECS - 2 * 86_400;
    state.lock().unwrap().held.as_mut().unwrap().2 = aged;
    age_listing(home.path(), aged);
    mcp_session(home.path(), "s-server");
    assert_eq!(
        state.lock().unwrap().uploads.len(),
        3,
        "renewed at server start"
    );
    let identity = &edge_identity_records(home.path(), "s-server")[0]["payload"];
    assert!(identity["listed_until"].is_string(), "{identity}");

    // Revoked: the hub drops the proof, the directory lists nothing, the
    // relay run learns it and uploads nothing, and the record says why.
    {
        let mut state = state.lock().unwrap();
        state.revoked = Some(("2026-09-06T08:00:00Z", "owner"));
        state.held = None;
    }
    assert!(usable_in_directory(&hub_url).is_empty());
    let output = commonmeasure(home.path(), &["relay"]);
    assert!(output.status.success());
    assert_eq!(state.lock().unwrap().uploads.len(), 3);
    session_start(home.path(), "s-revoked");
    let identity = &edge_identity_records(home.path(), "s-revoked")[0]["payload"];
    let unlisted = identity["unlisted"].as_str().expect("unlisted");
    assert!(unlisted.contains("revoked"), "{unlisted}");
    assert_eq!(state.lock().unwrap().uploads.len(), 3);

    server.stop();
}

/// What this build writes to `enrolment.json` loads in the 0.3.1 release,
/// whatever the directory proof does: after `connect` uploads one, after a
/// start renews it, and after a relay run learns a revocation. The listing
/// lives in its own file, which the release never reads.
///
/// Catches: a new member on the enrolment record, which the release refuses,
/// so a host still running the release binary against the same operator home
/// fails to start its mediated server.
#[test]
fn the_enrolment_record_keeps_the_shape_the_0_3_1_release_reads() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(HubState {
        proof_authority: Some("hub.example"),
        ..HubState::default()
    }));
    let mut server = hub(state.clone());
    let output = commonmeasure(home.path(), &["connect", &server.url(), "--token", TOKEN]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(state.lock().unwrap().uploads.len(), 1);
    assert!(home.path().join("directory-listing.json").exists());
    let released = released_0_3_1::load(home.path()).expect("0.3.1 reads the record connect wrote");
    assert_eq!(Some(released.key_id), state.lock().unwrap().key_id);

    let aged = now() + LIFETIME_SECS - 2 * 86_400;
    state.lock().unwrap().held.as_mut().unwrap().2 = aged;
    age_listing(home.path(), aged);
    session_start(home.path(), "s-renewed");
    assert_eq!(state.lock().unwrap().uploads.len(), 2);
    released_0_3_1::load(home.path()).expect("0.3.1 reads the record after a renewal");

    state.lock().unwrap().revoked = Some(("2026-09-06T08:00:00Z", "owner"));
    let output = commonmeasure(home.path(), &["relay"]);
    assert!(output.status.success());
    let released =
        released_0_3_1::load(home.path()).expect("0.3.1 reads the record after a revocation");
    assert_eq!(released.revoked_at.as_deref(), Some("2026-09-06T08:00:00Z"));

    let output = commonmeasure(home.path(), &["disconnect"]);
    assert!(output.status.success());
    assert!(
        !home.path().join("directory-listing.json").exists(),
        "the listing goes with the enrolment"
    );
    server.stop();
}

/// A refused upload keeps the enrolment, says why on `connect`'s account
/// and on every session record, and a session start within the hour does
/// not ask again.
#[test]
fn a_refused_directory_proof_leaves_the_enrolment_standing_and_the_record_says_why() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(HubState {
        proof_authority: Some("hub.example"),
        refuse_upload: Some((422, "created is later than now plus 60 seconds")),
        ..HubState::default()
    }));
    let mut server = hub(state.clone());
    let output = commonmeasure(home.path(), &["connect", &server.url(), "--token", TOKEN]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("directory proof: not accepted: the hub refused the proof (422)"),
        "{stdout}"
    );
    assert!(home.path().join("enrolment.json").exists());
    let uploads = state.lock().unwrap().uploads.len();
    assert!(uploads >= 1);

    session_start(home.path(), "s-refused");
    assert_eq!(
        state.lock().unwrap().uploads.len(),
        uploads,
        "a start within the hour of a failure does not try again"
    );
    let identity = &edge_identity_records(home.path(), "s-refused")[0]["payload"];
    let unlisted = identity["unlisted"].as_str().expect("unlisted");
    assert!(unlisted.contains("created is later than now"), "{unlisted}");
    assert!(usable_in_directory(&server.url()).is_empty());
    server.stop();
}

/// The hub's listing decision survives an accepted proof and a relay status
/// response, and the next session and local status use the stored statement.
#[test]
fn the_hubs_unlisted_reason_survives_enrolment_renewal_and_local_reads() {
    const REASON: &str = "release below the hub's floor; upgrade this edge";
    for on_renewal in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let state = Arc::new(Mutex::new(HubState {
            proof_authority: Some("hub.example"),
            listing_decision: (!on_renewal).then_some((false, Some(REASON))),
            ..HubState::default()
        }));
        let mut server = hub(state.clone());
        let output = commonmeasure(home.path(), &["connect", &server.url(), "--token", TOKEN]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if on_renewal {
            let aged = now() + LIFETIME_SECS - 2 * 86_400;
            {
                let mut state = state.lock().unwrap();
                state.held.as_mut().unwrap().2 = aged;
                state.upload_decision = Some((false, Some(REASON)));
            }
            age_listing(home.path(), aged);
            session_start(home.path(), "s-renewal-refused");
        }
        let listing = directory_listing(home.path());
        assert_eq!(listing["stated"]["listed"], false);
        assert_eq!(listing["stated"]["unlisted_reason"], REASON);
        let calls = state.lock().unwrap().status_keys.len();
        session_start(home.path(), "s-unlisted");
        let identity = &edge_identity_records(home.path(), "s-unlisted")[0]["payload"];
        assert_eq!(identity["unlisted"], format!("hub: {REASON}"));
        let output = commonmeasure(home.path(), &["status"]);
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains(&format!("hub: {REASON}")));
        let output = commonmeasure(home.path(), &["status", "--json"]);
        assert!(output.status.success());
        let status: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            status["directory_listing"]["unlisted"],
            format!("hub: {REASON}")
        );
        assert_eq!(
            state.lock().unwrap().status_keys.len(),
            calls,
            "local reads need no hub call"
        );

        // A current proof still learns a changed decision through relay status.
        state.lock().unwrap().listing_decision = Some((true, None));
        let uploads = state.lock().unwrap().uploads.len();
        let output = commonmeasure(home.path(), &["relay"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(state.lock().unwrap().uploads.len(), uploads);
        session_start(home.path(), "s-listed-again");
        assert!(
            edge_identity_records(home.path(), "s-listed-again")[0]["payload"]["listed_until"]
                .is_string()
        );
        server.stop();
    }
}

/// A release change requires an upload independently of the hub's listing
/// decision. Both start-time and relay refreshes must remember its acceptance.
fn check_release_refresh(recorded_release: Option<&str>, upload_due: bool) {
    for at_start in [true, false] {
        for listed in [true, false] {
            let home = tempfile::tempdir().unwrap();
            let state = Arc::new(Mutex::new(HubState {
                proof_authority: Some("hub.example"),
                listing_decision: Some((listed, (!listed).then_some("the hub withheld this key"))),
                ..HubState::default()
            }));
            let mut server = hub(state.clone());
            let output = commonmeasure(home.path(), &["connect", &server.url(), "--token", TOKEN]);
            assert!(output.status.success());
            assert_eq!(state.lock().unwrap().uploads.len(), 1);
            assert_eq!(
                directory_listing(home.path())["last_uploaded_release"],
                RELEASE
            );
            let mut listing = directory_listing(home.path());
            if let Some(release) = recorded_release {
                listing["last_uploaded_release"] = json!(release);
            } else {
                listing
                    .as_object_mut()
                    .unwrap()
                    .remove("last_uploaded_release");
            }
            std::fs::write(
                home.path().join("directory-listing.json"),
                listing.to_string(),
            )
            .unwrap();

            if at_start {
                session_start(home.path(), "s-release");
            } else {
                let output = commonmeasure(home.path(), &["relay"]);
                assert!(output.status.success());
            }
            let expected_uploads = if upload_due { 2 } else { 1 };
            assert_eq!(
                state.lock().unwrap().uploads.len(),
                expected_uploads,
                "release {recorded_release:?}, session start {at_start}, listed {listed}"
            );
            let listing = directory_listing(home.path());
            assert_eq!(listing["last_uploaded_release"], RELEASE);
            assert!(listing["stated"].get("last_uploaded_release").is_none());
            assert_eq!(
                state.lock().unwrap().uploads.last().unwrap()["release"],
                RELEASE
            );

            // A status read must retain the accepted release, and neither
            // entry point uploads again while this release's proof is current.
            assert!(commonmeasure(home.path(), &["relay"]).status.success());
            session_start(home.path(), "s-after-release");
            assert_eq!(state.lock().unwrap().uploads.len(), expected_uploads);
            assert_eq!(
                directory_listing(home.path())["last_uploaded_release"],
                RELEASE
            );
            server.stop();
        }
    }
}

#[test]
fn a_listing_without_a_release_uploads_at_session_start_and_relay() {
    check_release_refresh(None, true);
}

#[test]
fn a_listing_from_another_release_uploads_at_session_start_and_relay() {
    check_release_refresh(Some("0.0.0"), true);
}

#[test]
fn a_listing_from_this_release_does_not_upload_at_session_start_or_relay() {
    check_release_refresh(Some(RELEASE), false);
}

#[test]
fn a_400_or_422_proof_refusal_preserves_a_current_listing_and_records_the_failure() {
    for (status, reason) in [
        (400, "release is malformed: expected a semantic version"),
        (422, "created is later than now plus 60 seconds"),
    ] {
        for decision in [None, Some((true, None))] {
            let home = tempfile::tempdir().unwrap();
            let state = Arc::new(Mutex::new(HubState {
                proof_authority: Some("hub.example"),
                listing_decision: decision,
                ..HubState::default()
            }));
            let mut server = hub(state.clone());
            let output = commonmeasure(home.path(), &["connect", &server.url(), "--token", TOKEN]);
            assert!(output.status.success());
            let mut listing = directory_listing(home.path());
            let statement = listing["stated"].clone();
            listing["last_uploaded_release"] = json!("0.0.0");
            std::fs::write(
                home.path().join("directory-listing.json"),
                listing.to_string(),
            )
            .unwrap();
            state.lock().unwrap().refuse_upload = Some((status, reason));

            session_start(home.path(), "s-refused-release");
            assert_eq!(state.lock().unwrap().uploads.len(), 2);
            let identity = &edge_identity_records(home.path(), "s-refused-release")[0]["payload"];
            assert!(identity["listed_until"].is_string(), "{identity}");
            assert!(identity.get("unlisted").is_none(), "{identity}");
            let listing = directory_listing(home.path());
            assert_eq!(listing["stated"], statement);
            assert_eq!(listing["last_uploaded_release"], "0.0.0");
            assert_eq!(
                listing["failure"],
                format!("the hub refused the proof ({status}): {reason}")
            );
            assert_eq!(
                usable_in_directory(&server.url()),
                vec![state.lock().unwrap().key_id.clone().unwrap()]
            );
            let output = commonmeasure(home.path(), &["status", "--json"]);
            assert!(output.status.success());
            let local_status: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert!(local_status["directory_listing"]["listed_until"].is_string());

            session_start(home.path(), "s-backoff");
            assert_eq!(
                state.lock().unwrap().uploads.len(),
                2,
                "the failure retry delay still applies"
            );
            server.stop();
        }
    }
}

/// Connect, then make the held proof due only because the release changed,
/// so the next start or relay run uploads.
fn connected_with_upload_due(state: &Arc<Mutex<HubState>>, home: &Path) -> ServerHandle {
    let server = hub(state.clone());
    assert!(
        commonmeasure(home, &["connect", &server.url(), "--token", TOKEN])
            .status
            .success()
    );
    change_recorded_release(home);
    server
}

/// A 401, 404 or 409 in the hub's own error shape: the edge concludes the key
/// is unlisted, stores that as its conclusion beside the hub's statement, and
/// never presents it as something the hub stated.
#[test]
fn upload_refusals_of_401_404_and_409_from_the_hub_are_stored_as_the_edges_conclusion() {
    const REASON: &str = "the hub refused this key";
    for status in [401, 404, 409] {
        let home = tempfile::tempdir().unwrap();
        let state = Arc::new(Mutex::new(HubState {
            proof_authority: Some("hub.example"),
            ..HubState::default()
        }));
        let mut server = connected_with_upload_due(&state, home.path());
        state.lock().unwrap().refuse_upload = Some((status, REASON));
        session_start(home.path(), "s-key-refused");
        assert_eq!(state.lock().unwrap().uploads.len(), 2);
        let identity = &edge_identity_records(home.path(), "s-key-refused")[0]["payload"];
        let unlisted = identity["unlisted"].as_str().expect("unlisted").to_owned();
        assert!(unlisted.contains(REASON), "{unlisted}");
        if status == 401 {
            assert_eq!(identity["standing"], "revoked");
            assert!(
                commonmeasure_harness::identity::Identity::load(home.path())
                    .unwrap()
                    .signer()
                    .is_none()
            );
        } else {
            assert_eq!(identity["standing"], "enrolled");
            assert!(!unlisted.starts_with("hub:"), "{unlisted}");
            assert!(unlisted.contains("this edge concluded"), "{unlisted}");
            assert!(unlisted.contains(&format!("({status})")), "{unlisted}");
            let output = commonmeasure(home.path(), &["status", "--json"]);
            assert!(output.status.success());
            let local: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(local["directory_listing"]["unlisted"], unlisted.as_str());
        }
        assert!(identity.get("listed_until").is_none());
        let listing = directory_listing(home.path());
        assert_eq!(
            listing["stated"]["edge_conclusion"],
            json!({"status": status, "detail": REASON})
        );
        // Repeated for a reader of 0.4.2 or earlier, which ignores the
        // conclusion member and must still read the key as unlisted.
        assert_eq!(listing["stated"]["listed"], false);
        assert_eq!(listing["stated"]["unlisted_reason"], REASON);
        assert_eq!(listing["last_uploaded_release"], "0.0.0");
        assert_eq!(
            listing["failure"],
            format!("the hub refused the proof ({status}): {REASON}")
        );
        server.stop();
    }
}

/// A 401 or 404 the hub did not write, such as an ingress or proxy page, is
/// a failure to reach the hub: the listing stays as the hub last stated it,
/// and a 401 neither records revocation nor stops signing.
#[test]
fn a_401_or_404_not_in_the_hubs_shape_is_a_failure_to_reach_it_and_leaves_the_listing() {
    // The 2 KiB boundary falls inside a multi-byte character.
    let html = format!("<html>{}</html>", "界".repeat(4_000));
    let truncated = format!("{}… (truncated, {} bytes)", &html[..2046], html.len());
    let answers = [(html, truncated), (String::new(), "no body".to_owned())];
    for (status, (body, shown)) in [401, 404]
        .into_iter()
        .flat_map(|status| answers.clone().map(|answer| (status, answer)))
    {
        let home = tempfile::tempdir().unwrap();
        let state = Arc::new(Mutex::new(HubState {
            proof_authority: Some("hub.example"),
            ..HubState::default()
        }));
        let mut server = connected_with_upload_due(&state, home.path());
        let before = directory_listing(home.path());
        state.lock().unwrap().upload_error = Some((status, body));
        session_start(home.path(), "s-ingress");
        assert_eq!(state.lock().unwrap().uploads.len(), 2);
        let listing = directory_listing(home.path());
        assert_eq!(listing["stated"], before["stated"]);
        assert!(listing["stated"].get("edge_conclusion").is_none());
        assert_eq!(listing["last_uploaded_release"], "0.0.0");
        let failure = listing["failure"].as_str().expect("failure");
        assert!(failure.starts_with("the hub was not reached"), "{failure}");
        assert!(failure.contains(&format!("answered {status}")), "{failure}");
        assert!(failure.ends_with(&shown), "{failure}");
        let identity = &edge_identity_records(home.path(), "s-ingress")[0]["payload"];
        assert_eq!(identity["standing"], "enrolled", "{identity}");
        assert!(
            commonmeasure_harness::identity::Identity::load(home.path())
                .unwrap()
                .signer()
                .is_some()
        );
        assert!(
            !commonmeasure_harness::EnrolmentRecord::load(home.path())
                .unwrap()
                .unwrap()
                .is_revoked()
        );
        assert!(identity["listed_until"].is_string(), "{identity}");
        assert!(identity.get("unlisted").is_none(), "{identity}");
        let output = commonmeasure(home.path(), &["status", "--json"]);
        assert!(output.status.success());
        let local: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(local["directory_listing"]["listed_until"].is_string());
        session_start(home.path(), "s-ingress-backoff");
        assert_eq!(
            state.lock().unwrap().uploads.len(),
            2,
            "the failure retry delay still applies"
        );
        server.stop();
    }
}

const INGRESS_PAGE: &str = "<html><head><title>401 Authorization Required</title></head></html>";

/// Connect, then have the status route answered `status` with `body`, of
/// media type `media` when given, by something in front of the hub, and ask
/// for standing on a relay run or, `at_start`, on a session start whose
/// proof is due. Returns the relay's output or the listing's `failure`,
/// whichever carries the reason.
fn status_answered_in_front(
    home: &Path,
    status: u16,
    body: &str,
    media: Option<&'static str>,
    at_start: bool,
) -> String {
    let state = Arc::new(Mutex::new(HubState {
        proof_authority: Some("hub.example"),
        ..HubState::default()
    }));
    let mut server = hub(state.clone());
    assert!(
        commonmeasure(home, &["connect", &server.url(), "--token", TOKEN])
            .status
            .success()
    );
    state.lock().unwrap().status_raw = Some((status, body.to_owned(), media));
    let reason = if at_start {
        change_recorded_release(home);
        session_start(home, "s-status-ingress");
        directory_listing(home)["failure"]
            .as_str()
            .expect("failure")
            .to_owned()
    } else {
        let output = commonmeasure(home, &["relay"]);
        assert!(output.status.success());
        session_start(home, "s-status-ingress");
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    assert_eq!(
        state.lock().unwrap().uploads.len(),
        1,
        "only connect's upload"
    );
    server.stop();
    reason
}

/// A 401 on the status route that the hub did not write, such as an ingress
/// page, an empty answer from a wall in front of every route or an API
/// gateway's JSON, says nothing about the key: the edge stays enrolled and
/// signing.
fn a_401_not_in_the_hubs_shape_revokes_nothing(at_start: bool) {
    for (body, media, shown) in [
        (INGRESS_PAGE, None, INGRESS_PAGE),
        ("", None, "no body"),
        (
            r#"{"message":"Unauthorized"}"#,
            Some("application/json"),
            "Unauthorized",
        ),
    ] {
        let home = tempfile::tempdir().unwrap();
        let reason = status_answered_in_front(home.path(), 401, body, media, at_start);
        assert!(
            reason.contains(&format!(
                "the hub was not reached: the status request was answered 401 by something in \
                 front of it, in a shape the hub does not answer in: {shown}"
            )),
            "{reason}"
        );
        assert_standing_unchanged(home.path());
    }
}

/// The edge is still enrolled, signing and not revoked after a standing
/// check that `status_answered_in_front` ran.
fn assert_standing_unchanged(home: &Path) {
    let identity = &edge_identity_records(home, "s-status-ingress")[0]["payload"];
    assert_eq!(identity["standing"], "enrolled", "{identity}");
    let record = commonmeasure_harness::EnrolmentRecord::load(home)
        .unwrap()
        .unwrap();
    assert!(!record.is_revoked());
    assert!(record.revocation.is_none());
    assert!(
        commonmeasure_harness::identity::Identity::load(home)
            .unwrap()
            .signer()
            .is_some()
    );
}

#[test]
fn a_401_on_the_status_route_not_in_the_hubs_shape_revokes_nothing_on_a_relay_run() {
    a_401_not_in_the_hubs_shape_revokes_nothing(false);
}

#[test]
fn a_401_on_the_status_route_not_in_the_hubs_shape_revokes_nothing_at_session_start() {
    a_401_not_in_the_hubs_shape_revokes_nothing(true);
}

/// An answer on the status route other than 200 is the hub's own only in
/// its shape; in any other, whatever its status, it is described as the hub
/// not reached. Neither changes standing.
#[test]
fn an_answer_on_the_status_route_is_credited_to_the_hub_only_in_its_shape() {
    for at_start in [false, true] {
        for status in [403, 404, 502, 503] {
            for (body, shown) in [(INGRESS_PAGE, INGRESS_PAGE), ("", "no body")] {
                let home = tempfile::tempdir().unwrap();
                let reason = status_answered_in_front(home.path(), status, body, None, at_start);
                assert!(
                    reason.contains(&format!(
                        "the hub was not reached: the status request was answered {status} by \
                         something in front of it, in a shape the hub does not answer in: {shown}"
                    )),
                    "{status} {reason}"
                );
                assert_standing_unchanged(home.path());
            }
        }
        // The hub's own answers, in its shape and media type.
        for (status, detail) in [
            (403, "this credential lacks the telemetry:ingest scope"),
            (404, "no edge key is enrolled under this credential"),
        ] {
            let home = tempfile::tempdir().unwrap();
            let state = Arc::new(Mutex::new(HubState {
                proof_authority: Some("hub.example"),
                ..HubState::default()
            }));
            let mut server = hub(state.clone());
            assert!(
                commonmeasure(home.path(), &["connect", &server.url(), "--token", TOKEN])
                    .status
                    .success()
            );
            state.lock().unwrap().status_error = Some((status, detail));
            let reason = if at_start {
                change_recorded_release(home.path());
                session_start(home.path(), "s-status-ingress");
                directory_listing(home.path())["failure"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            } else {
                let output = commonmeasure(home.path(), &["relay"]);
                assert!(output.status.success());
                session_start(home.path(), "s-status-ingress");
                String::from_utf8_lossy(&output.stdout).into_owned()
            };
            assert!(
                reason.contains(&format!("the hub answered {status}: {detail}")),
                "{reason}"
            );
            assert!(!reason.contains("not reached"), "{reason}");
            assert_standing_unchanged(home.path());
            server.stop();
        }
    }
}

/// On the upload, any status the hub did not write in its own shape, such
/// as a WAF's 403 or a proxy's 502, is described as the hub not reached;
/// the hub's own 4xx keeps its wording as a refusal, and its own 429 or 5xx
/// is described as a fault from which nothing is concluded.
#[test]
fn an_upload_answer_not_in_the_hubs_shape_is_not_a_refusal_by_the_hub() {
    let answers = [
        (403, INGRESS_PAGE, INGRESS_PAGE),
        (502, "<html>Bad Gateway</html>", "<html>Bad Gateway</html>"),
        (503, "", "no body"),
    ];
    for (status, body, shown) in answers {
        let home = tempfile::tempdir().unwrap();
        let state = Arc::new(Mutex::new(HubState {
            proof_authority: Some("hub.example"),
            ..HubState::default()
        }));
        let mut server = connected_with_upload_due(&state, home.path());
        state.lock().unwrap().upload_error = Some((status, body.to_owned()));
        session_start(home.path(), "s-upload-ingress");
        assert_eq!(state.lock().unwrap().uploads.len(), 2);
        assert_eq!(
            directory_listing(home.path())["failure"],
            format!(
                "the hub was not reached: the upload was answered {status} by something in \
                 front of it, in a shape the hub does not answer in: {shown}"
            )
        );
        server.stop();
    }
    for (status, detail) in [
        (403, "the credential lacks the telemetry:ingest scope"),
        (429, "too many requests for this credential"),
        (500, "internal server error"),
        (503, "the identity documents need IDENTITY_ORIGIN"),
    ] {
        let home = tempfile::tempdir().unwrap();
        let state = Arc::new(Mutex::new(HubState {
            proof_authority: Some("hub.example"),
            ..HubState::default()
        }));
        let mut server = connected_with_upload_due(&state, home.path());
        state.lock().unwrap().refuse_upload = Some((status, detail));
        session_start(home.path(), "s-upload-hub");
        // A 429 or a 5xx rules on nothing; a 4xx is the hub's refusal.
        let failure = if status == 429 || status >= 500 {
            format!(
                "the hub could not take the proof ({status}): {detail}; nothing was concluded \
                 from it"
            )
        } else {
            format!("the hub refused the proof ({status}): {detail}")
        };
        assert_eq!(directory_listing(home.path())["failure"], failure);
        server.stop();
    }
}

/// The next status answer is the hub's statement and replaces the edge's
/// conclusion, whichever way the hub decides.
#[test]
fn a_later_status_answer_replaces_the_edges_conclusion() {
    for (decision, shown) in [
        (
            (false, Some("the hub withheld this key")),
            Some("hub: the hub withheld this key"),
        ),
        ((true, None), None),
    ] {
        let home = tempfile::tempdir().unwrap();
        let state = Arc::new(Mutex::new(HubState {
            proof_authority: Some("hub.example"),
            ..HubState::default()
        }));
        let mut server = connected_with_upload_due(&state, home.path());
        state.lock().unwrap().refuse_upload = Some((409, "the edge key is revoked"));
        assert!(commonmeasure(home.path(), &["relay"]).status.success());
        assert_eq!(
            directory_listing(home.path())["stated"]["edge_conclusion"]["status"],
            409
        );
        state.lock().unwrap().listing_decision = Some(decision);
        assert!(commonmeasure(home.path(), &["relay"]).status.success());
        assert_eq!(
            state.lock().unwrap().uploads.len(),
            2,
            "within the retry delay"
        );
        let listing = directory_listing(home.path());
        assert!(
            listing["stated"].get("edge_conclusion").is_none(),
            "{listing}"
        );
        assert_eq!(listing["stated"]["listed"], decision.0);
        let output = commonmeasure(home.path(), &["status", "--json"]);
        assert!(output.status.success());
        let local: Value = serde_json::from_slice(&output.stdout).unwrap();
        match shown {
            Some(shown) => assert_eq!(local["directory_listing"]["unlisted"], shown),
            None => assert!(
                local["directory_listing"]["listed_until"].is_string(),
                "{local}"
            ),
        }
        server.stop();
    }
}

/// Make a current held proof due only because this release has not uploaded.
fn change_recorded_release(home: &Path) {
    let mut listing = directory_listing(home);
    listing["last_uploaded_release"] = json!("0.0.0");
    std::fs::write(home.join("directory-listing.json"), listing.to_string()).unwrap();
}

#[test]
fn an_accepted_upload_with_an_unreadable_statement_records_the_release() {
    for answer in [
        json!({"directory_proof": {"authority": "hub.example", "lifetime_secs": LIFETIME_SECS, "listed": "false"}}).to_string(),
        json!({"directory_proof": {"authority": "hub.example", "lifetime_secs": LIFETIME_SECS, "unlisted_reason": 42}}).to_string(),
        "not JSON".to_owned(),
    ] {
        let home = tempfile::tempdir().unwrap();
        let state = Arc::new(Mutex::new(HubState {
            proof_authority: Some("hub.example"),
            listing_decision: Some((false, Some("the hub withheld this key"))),
            ..HubState::default()
        }));
        let mut server = hub(state.clone());
        assert!(commonmeasure(home.path(), &["connect", &server.url(), "--token", TOKEN]).status.success());
        change_recorded_release(home.path());
        let before = directory_listing(home.path())["stated"].clone();
        state.lock().unwrap().upload_answer = Some(answer);
        let output = commonmeasure(home.path(), &["relay"]);
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("signed and accepted by the hub"));
        let listing = directory_listing(home.path());
        assert_eq!(listing["stated"], before);
        assert_eq!(listing["last_uploaded_release"], RELEASE);
        assert!(listing["failure"].as_str().unwrap().contains("not readable"));
        assert_eq!(state.lock().unwrap().uploads.len(), 2);
        assert!(commonmeasure(home.path(), &["relay"]).status.success());
        session_start(home.path(), "s-after-unreadable");
        assert_eq!(state.lock().unwrap().uploads.len(), 2);
        server.stop();
    }
}

#[test]
fn a_statement_less_accepted_upload_clears_the_old_listing_decision() {
    for later_held in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let state = Arc::new(Mutex::new(HubState {
            proof_authority: Some("hub.example"),
            listing_decision: Some((false, Some("no release was reported"))),
            ..HubState::default()
        }));
        let mut server = hub(state.clone());
        assert!(
            commonmeasure(home.path(), &["connect", &server.url(), "--token", TOKEN])
                .status
                .success()
        );
        change_recorded_release(home.path());
        // Force either the uploaded or the pre-existing proof to expire later.
        let previous_expiry = now() + LIFETIME_SECS + if later_held { 600 } else { -600 };
        {
            let mut hub = state.lock().unwrap();
            hub.held.as_mut().unwrap().2 = previous_expiry;
            hub.upload_answer = Some(json!({"stored": !later_held}).to_string());
        }
        session_start(home.path(), "s-without-statement");
        let listing = directory_listing(home.path());
        assert!(listing["stated"].get("listed").is_none());
        assert!(listing["stated"].get("unlisted_reason").is_none());
        assert!(listing.get("failure").is_none());
        assert_eq!(listing["last_uploaded_release"], RELEASE);
        let expiry =
            chrono::DateTime::parse_from_rfc3339(listing["stated"]["expires_at"].as_str().unwrap())
                .unwrap()
                .timestamp();
        if later_held {
            assert_eq!(expiry, previous_expiry);
        } else {
            assert!(expiry > previous_expiry);
            assert_eq!(expiry, state.lock().unwrap().held.as_ref().unwrap().2);
        }
        let identity = &edge_identity_records(home.path(), "s-without-statement")[0]["payload"];
        assert!(identity["listed_until"].is_string());
        assert!(identity.get("unlisted").is_none());
        server.stop();
    }
}

#[test]
fn relay_retries_a_refused_release_only_upload_after_an_hour() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(HubState {
        proof_authority: Some("hub.example"),
        ..HubState::default()
    }));
    let mut server = hub(state.clone());
    assert!(
        commonmeasure(home.path(), &["connect", &server.url(), "--token", TOKEN])
            .status
            .success()
    );
    change_recorded_release(home.path());
    state.lock().unwrap().refuse_upload = Some((422, "created is later than now plus 60 seconds"));
    assert!(commonmeasure(home.path(), &["relay"]).status.success());
    assert_eq!(state.lock().unwrap().uploads.len(), 2);
    let failed = directory_listing(home.path());
    let mut recent = failed.clone();
    // Move only the recorded attempt time; no clock or network test double.
    recent["checked_at"] = json!(rfc3339(now() - 1_800));
    std::fs::write(
        home.path().join("directory-listing.json"),
        recent.to_string(),
    )
    .unwrap();
    state.lock().unwrap().listing_decision = Some((false, Some("the hub withheld this key")));
    for _ in 0..3 {
        assert!(commonmeasure(home.path(), &["relay"]).status.success());
        let listing = directory_listing(home.path());
        assert_eq!(listing["checked_at"], recent["checked_at"]);
        assert_eq!(listing["failure"], failed["failure"]);
        assert_eq!(listing["last_uploaded_release"], "0.0.0");
        assert_eq!(
            listing["stated"]["unlisted_reason"],
            "the hub withheld this key"
        );
    }
    assert_eq!(state.lock().unwrap().uploads.len(), 2);
    let mut elapsed = directory_listing(home.path());
    elapsed["checked_at"] = json!(rfc3339(now() - 3_600));
    std::fs::write(
        home.path().join("directory-listing.json"),
        elapsed.to_string(),
    )
    .unwrap();
    assert!(commonmeasure(home.path(), &["relay"]).status.success());
    assert_eq!(state.lock().unwrap().uploads.len(), 3);
    assert_ne!(
        directory_listing(home.path())["checked_at"],
        elapsed["checked_at"]
    );
    assert!(commonmeasure(home.path(), &["relay"]).status.success());
    assert_eq!(state.lock().unwrap().uploads.len(), 3);
    server.stop();
}

#[test]
fn relay_keeps_retrying_refused_proofs_due_by_age_or_expiry() {
    for remaining in [LIFETIME_SECS - 86_401, 7_199] {
        let home = tempfile::tempdir().unwrap();
        let state = Arc::new(Mutex::new(HubState {
            proof_authority: Some("hub.example"),
            ..HubState::default()
        }));
        let mut server = hub(state.clone());
        assert!(
            commonmeasure(home.path(), &["connect", &server.url(), "--token", TOKEN])
                .status
                .success()
        );
        change_recorded_release(home.path());
        state.lock().unwrap().refuse_upload = Some((422, "the proof was refused"));
        assert!(commonmeasure(home.path(), &["relay"]).status.success());
        assert_eq!(state.lock().unwrap().uploads.len(), 2);
        state.lock().unwrap().held.as_mut().unwrap().2 = now() + remaining;
        for expected in 3..=5 {
            assert!(commonmeasure(home.path(), &["relay"]).status.success());
            assert_eq!(state.lock().unwrap().uploads.len(), expected);
            assert_eq!(
                directory_listing(home.path())["last_uploaded_release"],
                "0.0.0"
            );
        }
        server.stop();
    }
}

/// A hub that serves its directory from another authority than the origin
/// this edge enrolled under gets no proof: the edge signs only for the
/// origin its requests name.
#[test]
fn no_proof_is_made_for_an_authority_the_edge_did_not_enrol_under() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(HubState {
        proof_authority: Some("elsewhere.example"),
        ..HubState::default()
    }));
    let mut server = hub(state.clone());
    let output = commonmeasure(home.path(), &["connect", &server.url(), "--token", TOKEN]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("serves its key directory from elsewhere.example"),
        "{stdout}"
    );
    assert!(state.lock().unwrap().uploads.is_empty());
    server.stop();
}

#[test]
fn disconnect_with_the_hub_unreachable_removes_the_files_and_says_the_key_must_be_revoked_there() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(HubState::default()));
    let mut server = hub(state.clone());
    let hub_url = server.url();
    let output = commonmeasure(home.path(), &["connect", &hub_url, "--token", TOKEN]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let key_id = state.lock().unwrap().key_id.clone().unwrap();

    // A second connect while enrolled is refused; the key is unchanged.
    let output = commonmeasure(home.path(), &["connect", &hub_url, "--token", TOKEN]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("already enrolled"));
    assert_eq!(state.lock().unwrap().exchange_bodies.len(), 1);

    server.stop();
    let output = commonmeasure(home.path(), &["disconnect"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains(&format!(
            "edge key {key_id} and its ingest key were not revoked at {hub_url}"
        )),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "revoke both on the hub's API keys page: the edge key {key_id} and the ingest key"
        )),
        "{stdout}"
    );
    for file in ["relay.json", "edge-key.json", "enrolment.json"] {
        assert!(
            !home.path().join(file).exists(),
            "{file} survived disconnect"
        );
    }
}

/// A token the hub refuses leaves nothing behind: no key, no record, no
/// relay configuration.
/// `connect --managed` enrols and, under the ingest key it just received,
/// reads the hub's policy signer and pins it in `deployment.json`, then
/// makes a first policy synchronisation. Plain `connect` writes no
/// deployment file: enrolment alone changes no policy mode.
#[test]
fn connect_with_managed_pins_the_hubs_signer_and_makes_a_first_policy_sync() {
    let state = Arc::new(Mutex::new(HubState::default()));
    let server = hub(state.clone());
    let hub_url = server.url();

    let local = tempfile::tempdir().unwrap();
    let output = commonmeasure(local.path(), &["connect", &hub_url, "--token", TOKEN]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !local.path().join("deployment.json").exists(),
        "enrolment without --managed pins nothing"
    );
    assert!(state.lock().unwrap().signer_keys.is_empty());

    let home = tempfile::tempdir().unwrap();
    let output = commonmeasure(
        home.path(),
        &["connect", &hub_url, "--token", TOKEN, "--managed"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "enrolled while awaiting the first revision: {stdout}"
    );

    let deployment: Value =
        serde_json::from_slice(&std::fs::read(home.path().join("deployment.json")).unwrap())
            .unwrap();
    assert_eq!(deployment["mode"], json!("managed"));
    assert_eq!(deployment["signer"]["key_id"], json!("hub-policy-test"));
    assert_eq!(deployment["signer"]["algorithm"], json!("ed25519"));
    assert_eq!(deployment["signer"]["public_key"], json!(SIGNER_PUBLIC_KEY));
    assert_eq!(
        deployment["policy_url"],
        json!(format!("{hub_url}/api/v1/policy/desired"))
    );
    assert_eq!(
        deployment["organisation"],
        json!("11111111-1111-1111-1111-111111111111")
    );
    assert_eq!(
        state.lock().unwrap().signer_keys,
        vec![Some(API_KEY.to_owned())],
        "the signer was read under the ingest key, not anonymously"
    );

    // What was said: the pin, and the first synchronisation's outcome. This
    // hub has published no revision, so its desired route answers 404, the
    // synchronisation activates nothing, and enrolment succeeds while
    // explicitly awaiting the first revision; the pin stands.
    assert!(
        stdout.contains("deployment  managed: signer hub-policy-test pinned in"),
        "{stdout}"
    );
    assert!(stdout.contains("first policy sync:"), "{stdout}");
    assert!(stdout.contains("outcome       no_revision"), "{stdout}");
    assert!(
        stdout.contains("waiting for the organisation's first policy revision"),
        "{stdout}"
    );
    assert!(!home.path().join("policy.json").exists());
    assert!(!home.path().join("managed/last-known-good.json").exists());
    let sync = commonmeasure(home.path(), &["policy", "sync"]);
    assert!(!sync.status.success(), "absence is not convergence");
    let output = commonmeasure(home.path(), &["status", "--json"]);
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["deployment_mode"], json!("managed"), "{status}");
    assert_eq!(
        status["desired"]["outcome"],
        json!("no_revision"),
        "{status}"
    );
    assert!(status["applied"]["revision"].is_null(), "{status}");

    // Leaving the hub takes the deployment pinned to it along.
    let output = commonmeasure(home.path(), &["disconnect"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !home.path().join("deployment.json").exists(),
        "the deployment pinned to the hub being left survived disconnect"
    );
    assert!(
        stdout.contains("deployment.json pinned this hub's policy"),
        "{stdout}"
    );
}

/// The hub's signer can carry the absolute `policy_url` built from its
/// public origin; the edge pins that, not the address it was typed under,
/// because the policy endpoint verifies signatures over that origin alone.
#[test]
fn connect_with_managed_prefers_the_hubs_absolute_policy_url_over_the_typed_address() {
    let state = Arc::new(Mutex::new(HubState {
        signer_policy_url: Some("https://hub.example/api/v1/policy/desired"),
        ..HubState::default()
    }));
    let server = hub(state.clone());
    let hub_url = server.url();
    let home = tempfile::tempdir().unwrap();
    let output = commonmeasure(
        home.path(),
        &["connect", &hub_url, "--token", TOKEN, "--managed"],
    );
    let deployment: Value =
        serde_json::from_slice(&std::fs::read(home.path().join("deployment.json")).unwrap())
            .unwrap();
    assert_eq!(
        deployment["policy_url"],
        json!("https://hub.example/api/v1/policy/desired"),
        "the hub's own URL, not {hub_url} plus the path"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("policy from https://hub.example/api/v1/policy/desired"),
        "{stdout}"
    );
}

/// A hub whose signer names a plain http policy URL off the machine is
/// refused at pin time by the rule synchronisation applies: the edge stays
/// enrolled and local, the exit is non-zero and nothing is pinned.
#[test]
fn connect_with_managed_refuses_a_plain_http_policy_url_off_the_machine() {
    let state = Arc::new(Mutex::new(HubState {
        signer_policy_url: Some("http://hub.internal/api/v1/policy/desired"),
        ..HubState::default()
    }));
    let server = hub(state.clone());
    let hub_url = server.url();
    let home = tempfile::tempdir().unwrap();
    let output = commonmeasure(
        home.path(),
        &["connect", &hub_url, "--token", TOKEN, "--managed"],
    );
    assert!(!output.status.success(), "a refused pin must exit non-zero");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("deployment  not pinned:"), "{stdout}");
    assert!(
        stderr.contains(
            "policy_url at http://hub.internal must be an https URL, or http to a loopback \
             origin"
        ),
        "{stderr}"
    );
    assert!(
        !home.path().join("deployment.json").exists(),
        "nothing was pinned"
    );
    assert!(
        home.path().join("enrolment.json").exists(),
        "the enrolment stands"
    );
}

/// A hub that does not let the ingest key read its signer leaves the edge
/// enrolled and local, and says so with a non-zero exit: nothing is pinned.
#[test]
fn connect_with_managed_against_a_hub_that_refuses_the_signer_enrols_and_exits_non_zero() {
    let state = Arc::new(Mutex::new(HubState {
        refuse_signer: true,
        ..HubState::default()
    }));
    let server = hub(state.clone());
    let hub_url = server.url();
    let home = tempfile::tempdir().unwrap();
    let output = commonmeasure(
        home.path(),
        &["connect", &hub_url, "--token", TOKEN, "--managed"],
    );
    assert!(!output.status.success(), "a refused pin must exit non-zero");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("deployment  not pinned:"), "{stdout}");
    assert!(
        stderr.contains("did not let this edge read the policy signer (401"),
        "{stderr}"
    );
    assert!(
        !home.path().join("deployment.json").exists(),
        "nothing was pinned"
    );
    assert!(
        home.path().join("enrolment.json").exists(),
        "the enrolment stands"
    );
    assert_eq!(
        state.lock().unwrap().signer_keys,
        vec![Some(API_KEY.to_owned())]
    );
}

/// A deployment that names another hub is not this hub's to remove.
#[test]
fn disconnect_keeps_a_deployment_pinned_to_another_hub() {
    let state = Arc::new(Mutex::new(HubState::default()));
    let server = hub(state.clone());
    let hub_url = server.url();
    let home = tempfile::tempdir().unwrap();
    let output = commonmeasure(home.path(), &["connect", &hub_url, "--token", TOKEN]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::write(
        home.path().join("deployment.json"),
        json!({
            "mode": "managed",
            "signer": {"key_id": "other-hub-policy", "algorithm": "ed25519",
                       "public_key": SIGNER_PUBLIC_KEY},
            "policy_url": "https://other-hub.example/api/v1/policy/desired",
            "organisation": "11111111-1111-1111-1111-111111111111",
        })
        .to_string(),
    )
    .unwrap();
    let output = commonmeasure(home.path(), &["disconnect"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(home.path().join("deployment.json").exists(), "{stdout}");
    assert!(
        stdout.contains("deployment.json names another hub's policy"),
        "{stdout}"
    );
}

/// After the hub revokes the key, the status route and the receiver both
/// answer 401. The relay prints the key's standing, the revocation, on its
/// own line before the delivery failure, and exits non-zero with the spool
/// intact.
#[test]
fn a_revoked_edges_relay_prints_the_revocation_before_the_delivery_failure() {
    let state = Arc::new(Mutex::new(HubState::default()));
    let server = hub(state.clone());
    let hub_url = server.url();
    let home = tempfile::tempdir().unwrap();
    let output = commonmeasure(home.path(), &["connect", &hub_url, "--token", TOKEN]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let key_id = state.lock().unwrap().key_id.clone().unwrap();
    record_cleared_crossing(home.path(), "s-revoked", "https://www.example.com/page");
    state.lock().unwrap().refuse_key = true;

    let output = commonmeasure(home.path(), &["relay"]);
    assert!(
        !output.status.success(),
        "delivery under a refused key must fail"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.starts_with(&format!("edge key {key_id}: revoked")),
        "the standing comes first, on stdout: {stdout}"
    );
    assert!(stderr.contains("delivery to"), "{stderr}");
    assert!(stderr.contains("401"), "{stderr}");
    assert!(
        home.path().join("relay/spool").exists(),
        "undelivered batches stay spooled"
    );
}

#[test]
fn a_refused_exchange_writes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(HubState::default()));
    let mut server = hub(state);
    let output = commonmeasure(
        home.path(),
        &["connect", &server.url(), "--token", "et_wrong"],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("refused the enrolment (401)"), "{stderr}");
    assert!(stderr.contains("enrolment token is unknown"), "{stderr}");
    for file in ["relay.json", "edge-key.json", "enrolment.json"] {
        assert!(!home.path().join(file).exists(), "{file} was written");
    }
    server.stop();
}

/// `connect` writes only a `relay.json` the relay loads. A hub URL with
/// credentials, a query or a fragment is refused before any request; a
/// telemetry path from the hub that makes such a receiver is refused after
/// the exchange and before anything is stored. Either way the home is left
/// empty, and a clean hub URL enrols with a configuration that loads.
#[test]
fn connect_writes_only_a_relay_config_the_relay_loads() {
    let state = Arc::new(Mutex::new(HubState::default()));
    let mut server = hub(state.clone());
    let clean = server.url();
    for (hub_url, fault) in [
        (clean.replacen("http://", "http://user@", 1), "credentials"),
        (
            clean.replacen("http://", "http://user:pass@", 1),
            "credentials",
        ),
        (format!("{clean}/?tenant=a"), "query or fragment"),
        (format!("{clean}#hub"), "query or fragment"),
    ] {
        let home = tempfile::tempdir().unwrap();
        let output = commonmeasure(home.path(), &["connect", &hub_url, "--token", TOKEN]);
        assert!(!output.status.success(), "{hub_url} enrolled");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(fault), "{hub_url}: {stderr}");
        assert!(
            stderr
                .contains("Give the hub's address alone, with no credentials, query or fragment."),
            "{hub_url}: {stderr}"
        );
        assert!(!stderr.contains("api_key"), "{hub_url}: {stderr}");
        // The refusal names the hub's origin, never the parts it refuses.
        for part in ["user@", "pass@", "tenant=a", "#hub"] {
            assert!(!stderr.contains(part), "{hub_url}: {stderr}");
        }
        assert!(
            stderr.contains("nothing was written"),
            "{hub_url}: {stderr}"
        );
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
    }
    assert_eq!(state.lock().unwrap().requests, 0);

    for (path, fault) in [
        ("/api/v1/telemetry?tenant=a", "query or fragment"),
        ("/api/v1/telemetry#x", "query or fragment"),
    ] {
        state.lock().unwrap().telemetry_path = Some(path);
        let home = tempfile::tempdir().unwrap();
        let output = commonmeasure(home.path(), &["connect", &clean, "--token", TOKEN]);
        assert!(!output.status.success(), "{path} enrolled");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(fault), "{path}: {stderr}");
        assert!(stderr.contains("Nothing was stored"), "{path}: {stderr}");
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
    }
    state.lock().unwrap().telemetry_path = None;

    let home = tempfile::tempdir().unwrap();
    let output = commonmeasure(home.path(), &["connect", &clean, "--token", TOKEN]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config = commonmeasure_harness::relay_config::RelayConfig::load(home.path())
        .expect("the relay loads what connect wrote")
        .expect("connect wrote relay.json");
    assert_eq!(config.receiver, format!("{clean}/api/v1/telemetry"));
    assert_eq!(config.api_key.as_deref(), Some(API_KEY));
    assert_eq!(config.suppliers, None);
    server.stop();
}

/// The whole enrolment life against a running Common Measure Hub, driven
/// by this binary: the real exchange, the key in the hub's directory with a
/// member it signed that verifies under the per-key rule, revocation from
/// the hub's API taking it out, the next relay run learning it, and
/// disconnect. Ignored by default because it needs a hub. To run it:
///
/// 1. Run a Common Measure Hub server with its database migrated and
///    `IDENTITY_ORIGIN` set, with an organisation whose owner has a
///    session.
/// 2. Export `COMMONMEASURE_TEST_HUB` (the server's base URL, e.g.
///    `http://127.0.0.1:8080`) and `COMMONMEASURE_TEST_HUB_SESSION` (the
///    owner's raw session token, sent as the `__Host-session` cookie).
/// 3. `cargo test -p commonmeasure-cli --test connect_e2e -- --ignored --nocapture`
///
/// It prints a transcript with every secret redacted.
#[test]
#[ignore = "requires a running hub server (see the doc comment for how to run it)"]
fn a_real_hub_enrols_revokes_and_disconnects_this_edge() {
    let hub_url = std::env::var("COMMONMEASURE_TEST_HUB")
        .expect("COMMONMEASURE_TEST_HUB names the running hub")
        .trim_end_matches('/')
        .to_owned();
    let cookie = format!(
        "__Host-session={}",
        std::env::var("COMMONMEASURE_TEST_HUB_SESSION")
            .expect("COMMONMEASURE_TEST_HUB_SESSION is the owner's session token")
    );
    let home = tempfile::tempdir().unwrap();
    let scratch = home.path().display().to_string();
    let redact = |text: &str| text.replace(&scratch, "<home>");

    let owner = |method: &str, path: &str, body: Option<Value>| -> (u16, Value) {
        let mut request = match body {
            Some(body) => Request::post(path, body.to_string().into_bytes(), "application/json"),
            None => Request::get(path),
        };
        request.method = method.to_owned();
        request.headers.set("Cookie", &cookie);
        let response = send(&format!("{hub_url}{path}"), request).expect("the hub answers");
        let body = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
        (response.status, body)
    };
    // The directory as a verifier reads it: the body, and the keys the
    // per-key rule lets it use, checked against the authority the hub
    // serves it from with no hub function involved.
    let directory = |authority: &str| -> (Value, Vec<String>) {
        let response = send(
            &format!("{hub_url}/.well-known/http-message-signatures-directory"),
            Request::get("/"),
        )
        .expect("the hub answers");
        assert_eq!(response.status, 200);
        let body = String::from_utf8_lossy(&response.body).into_owned();
        let usable = webbotauth::usable_keys(
            &body,
            response.headers.get("Signature-Input"),
            response.headers.get("Signature"),
            authority,
            now(),
        )
        .expect("a directory a verifier can read");
        for (kid, reason) in &usable.refused {
            println!("directory key {kid} not usable: {reason}");
        }
        (
            serde_json::from_str(&body).expect("json"),
            usable.keys.into_keys().collect(),
        )
    };
    let kids = |directory: &Value| -> Vec<String> {
        directory["keys"]
            .as_array()
            .expect("keys")
            .iter()
            .filter_map(|key| key["kid"].as_str().map(str::to_owned))
            .collect()
    };

    println!("## 1. An owner mints an enrolment token through the hub's API");
    let (status, minted) = owner(
        "POST",
        "/api/v1/enrolment/tokens",
        Some(json!({"name": "real-hub-run"})),
    );
    assert_eq!(status, 201, "{minted}");
    let token = minted["value"].as_str().expect("token").to_owned();
    println!(
        "POST /api/v1/enrolment/tokens -> 201 name={} expires_at={} value=et_<redacted>",
        minted["name"], minted["expires_at"]
    );

    println!("\n## 2. Before connect, the relay refuses: no receiver");
    let output = commonmeasure(home.path(), &["relay"]);
    assert!(!output.status.success());
    print!("{}", redact(&String::from_utf8_lossy(&output.stderr)));

    println!("\n## 3. commonmeasure connect <hub-url> --token <token>");
    let output = commonmeasure(home.path(), &["connect", &hub_url, "--token", &token]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{}", redact(&stdout));
    let enrolment: Value =
        serde_json::from_slice(&std::fs::read(home.path().join("enrolment.json")).unwrap())
            .unwrap();
    let key_id = enrolment["key_id"].as_str().expect("key id").to_owned();
    assert!(!stdout.contains("ak_"), "the ingest key was printed");

    println!(
        "\n## 4. The key is in the hub's directory with a member it signed, and in the owner's \
         listing"
    );
    let origin = enrolment["identity"]["origin"].as_str().expect("origin");
    let authority = origin
        .split_once("://")
        .map(|(_, rest)| rest.trim_end_matches('/'))
        .expect("an absolute origin")
        .to_owned();
    assert!(
        serde_json::from_slice::<Value>(
            &std::fs::read(home.path().join("directory-listing.json")).unwrap()
        )
        .unwrap()["stated"]["expires_at"]
            .is_string(),
        "connect recorded the proof the hub holds"
    );
    let (listed, usable) = directory(&authority);
    assert!(kids(&listed).contains(&key_id));
    assert!(
        usable.contains(&key_id),
        "the enrolled key is listed with a member that verifies under it"
    );
    println!("directory kids: {:?}; usable: {usable:?}", kids(&listed));
    let (status, keys) = owner("GET", "/api/v1/enrolment/keys", None);
    assert_eq!(status, 200);
    let ours = keys
        .as_array()
        .expect("keys")
        .iter()
        .find(|key| key["key_id"] == json!(key_id))
        .expect("the enrolled key is listed");
    println!(
        "GET /api/v1/enrolment/keys -> name={} api_key_name={} revoked_at={}",
        ours["name"], ours["api_key_name"], ours["revoked_at"]
    );

    println!("\n## 5. A session names the key id; a cleared crossing relays under the ingest key");
    session_start(home.path(), "real-1");
    for record in edge_identity_records(home.path(), "real-1") {
        println!("{}", redact(&record.to_string()));
    }
    record_cleared_crossing(home.path(), "real-1", "https://www.gov.uk/cap");
    let output = commonmeasure(home.path(), &["relay"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{}", redact(&String::from_utf8_lossy(&output.stdout)));

    println!("\n## 6. The owner revokes the key at the hub; the next relay run learns it");
    let (status, _) = owner("DELETE", &format!("/api/v1/enrolment/keys/{key_id}"), None);
    assert_eq!(status, 204);
    println!("DELETE /api/v1/enrolment/keys/<key_id> -> 204");
    let (after, usable) = directory(&authority);
    assert!(!kids(&after).contains(&key_id));
    assert!(!usable.contains(&key_id));
    println!("directory kids: {:?}; usable: {usable:?}", kids(&after));
    let output = commonmeasure(home.path(), &["relay"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{}", redact(&String::from_utf8_lossy(&output.stdout)));
    session_start(home.path(), "real-2");
    let identity = edge_identity_records(home.path(), "real-2");
    assert_eq!(identity[0]["payload"]["standing"], json!("revoked"));
    println!("{}", redact(&identity[0].to_string()));

    println!("\n## 7. commonmeasure disconnect");
    let output = commonmeasure(home.path(), &["disconnect"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{}", redact(&String::from_utf8_lossy(&output.stdout)));
    let (_, keys) = owner("GET", "/api/v1/enrolment/keys", None);
    let ours = keys
        .as_array()
        .expect("keys")
        .iter()
        .find(|key| key["key_id"] == json!(key_id))
        .expect("the key stays listed as revoked");
    assert!(ours["api_key_revoked_at"].is_string());
    println!(
        "GET /api/v1/enrolment/keys -> revocation={} api_key_revoked_at={}",
        ours["revocation"], ours["api_key_revoked_at"]
    );
    for file in ["relay.json", "edge-key.json", "enrolment.json"] {
        assert!(!home.path().join(file).exists());
    }
}

/// Only the explicit absence response can finish enrolment without a revision.
#[test]
fn managed_connect_still_fails_on_errors_and_keeps_the_local_policy() {
    for response in [
        (404, r#"{"detail":"not found"}"#),
        (404, "not JSON"),
        (401, r#"{"detail":"a valid credential is required"}"#),
        (403, r#"{"detail":"forbidden"}"#),
        (
            500,
            r#"{"detail":"no policy revision has been published for this organisation"}"#,
        ),
        (
            200,
            r#"{"detail":"no policy revision has been published for this organisation"}"#,
        ),
    ] {
        let server = hub(Arc::new(Mutex::new(HubState {
            policy_response: Some(response),
            ..HubState::default()
        })));
        let home = tempfile::tempdir().unwrap();
        let policy = br#"{"policy_mode":"strict"}"#;
        std::fs::write(home.path().join("policy.json"), policy).unwrap();
        let output = commonmeasure(
            home.path(),
            &["connect", &server.url(), "--token", TOKEN, "--managed"],
        );
        assert!(!output.status.success(), "{response:?}");
        assert!(home.path().join("deployment.json").exists());
        assert_eq!(
            std::fs::read(home.path().join("policy.json")).unwrap(),
            policy
        );
        assert!(
            !commonmeasure_harness::EnrolmentRecord::load(home.path())
                .unwrap()
                .unwrap()
                .is_revoked()
        );
        assert!(!home.path().join("managed/last-known-good.json").exists());
    }
}

/// Fault injection at the HTTP boundary models member removal. It establishes
/// edge handling of the hub's 401, not the hub's member-removal transaction.
#[test]
fn member_removal_stops_signing_on_relay_and_start_time_standing_checks() {
    const DETAIL: &str = "the member was removed";
    for at_start in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let state = Arc::new(Mutex::new(HubState {
            proof_authority: Some("hub.example"),
            ..HubState::default()
        }));
        let mut server = hub(state.clone());
        assert!(
            commonmeasure(home.path(), &["connect", &server.url(), "--token", TOKEN])
                .status
                .success()
        );
        assert!(
            commonmeasure_harness::identity::Identity::load(home.path())
                .unwrap()
                .signer()
                .is_some()
        );
        state.lock().unwrap().status_error = Some((401, DETAIL));
        if at_start {
            change_recorded_release(home.path());
        } else {
            let output = commonmeasure(home.path(), &["relay"]);
            assert!(String::from_utf8_lossy(&output.stdout).contains(DETAIL));
        }
        session_start(home.path(), "s-member-removed");
        let identity = &edge_identity_records(home.path(), "s-member-removed")[0]["payload"];
        assert_eq!(identity["standing"], "revoked");
        assert_eq!(identity["revocation"], format!("hub: {DETAIL}"));
        assert!(
            identity["revoked_at"].is_null(),
            "the hub supplied no revocation time"
        );
        let record = commonmeasure_harness::EnrolmentRecord::load(home.path())
            .unwrap()
            .unwrap();
        assert!(record.revocation_learnt_at.is_some());
        assert!(
            commonmeasure_harness::identity::Identity::load(home.path())
                .unwrap()
                .signer()
                .is_none()
        );
        let output = commonmeasure(home.path(), &["status", "--json"]);
        assert!(output.status.success());
        let status: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(status["egress"]["key_standing"], "revoked");
        assert!(
            status["directory_listing"]["unlisted"]
                .as_str()
                .unwrap()
                .contains(DETAIL)
        );
        let output = commonmeasure(home.path(), &["status"]);
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains("revoked") && text.contains(DETAIL), "{text}");
        let learnt = record.revocation_learnt_at;
        commonmeasure(home.path(), &["relay"]);
        assert_eq!(
            commonmeasure_harness::EnrolmentRecord::load(home.path())
                .unwrap()
                .unwrap()
                .revocation_learnt_at,
            learnt
        );
        server.stop();
    }
}

#[test]
fn server_and_transport_failures_leave_the_enrolled_key_signing() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(HubState {
        proof_authority: Some("hub.example"),
        ..HubState::default()
    }));
    let mut server = hub(state.clone());
    assert!(
        commonmeasure(home.path(), &["connect", &server.url(), "--token", TOKEN])
            .status
            .success()
    );
    for status in [403, 404, 500, 503] {
        state.lock().unwrap().status_error = Some((status, "standing unavailable"));
        assert!(commonmeasure(home.path(), &["relay"]).status.success());
        assert!(
            !commonmeasure_harness::EnrolmentRecord::load(home.path())
                .unwrap()
                .unwrap()
                .is_revoked()
        );
        assert!(
            commonmeasure_harness::identity::Identity::load(home.path())
                .unwrap()
                .signer()
                .is_some()
        );
    }
    server.stop();
    assert!(commonmeasure(home.path(), &["relay"]).status.success());
    assert!(
        !commonmeasure_harness::EnrolmentRecord::load(home.path())
            .unwrap()
            .unwrap()
            .is_revoked()
    );
    assert!(
        commonmeasure_harness::identity::Identity::load(home.path())
            .unwrap()
            .signer()
            .is_some()
    );
}

#[test]
fn cleartext_remote_connect_is_refused_before_any_request_or_local_write() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let local_unspecified = format!("http://0.0.0.0:{}", listener.local_addr().unwrap().port());
    for url in ["http://hub.example", local_unspecified.as_str()] {
        let home = tempfile::tempdir().unwrap();
        let output = commonmeasure(home.path(), &["connect", url, "--token", TOKEN]);
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("must be https"), "{error}");
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
    }
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

/// A home enrolled before `connect` applied the transport rule, holding a
/// cleartext hub. The stored URL is `http://0.0.0.0:<port>`, which reaches
/// the loopback double, so a guard that stopped refusing would show up as a
/// request here rather than as a lookup failure.
fn legacy_cleartext_home() -> (
    tempfile::TempDir,
    Arc<Mutex<HubState>>,
    ServerHandle,
    String,
) {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(HubState {
        proof_authority: Some("hub.example"),
        ..HubState::default()
    }));
    let server = hub(state.clone());
    assert!(
        commonmeasure(
            home.path(),
            &["connect", &server.url(), "--token", TOKEN, "--managed"]
        )
        .status
        .success()
    );
    let port = server
        .url()
        .rsplit(':')
        .next()
        .unwrap()
        .trim_end_matches('/')
        .to_owned();
    let cleartext = format!("http://0.0.0.0:{port}");
    let mut record = commonmeasure_harness::EnrolmentRecord::load(home.path())
        .unwrap()
        .unwrap();
    record.hub = cleartext.clone();
    record.store(home.path()).unwrap();
    std::fs::write(
        home.path().join("relay.json"),
        json!({"receiver": format!("{cleartext}/api/v1/telemetry"), "api_key": API_KEY})
            .to_string(),
    )
    .unwrap();
    (home, state, server, cleartext)
}

#[test]
fn a_legacy_cleartext_hub_is_refused_on_every_enrolment_request_path() {
    let (home, state, mut server, cleartext) = legacy_cleartext_home();
    let before = state.lock().unwrap().requests;

    // The relay refuses before it opens the spool or its state.
    std::fs::remove_dir_all(home.path().join("relay")).unwrap();
    let output = commonmeasure(home.path(), &["relay"]);
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("neither https nor http to a loopback origin")
            && error.contains("run `commonmeasure connect` with an https hub"),
        "{error}"
    );
    assert!(
        !home.path().join("relay").exists(),
        "a refused relay left relay state"
    );

    let error = commonmeasure_relay::check_standing(home.path(), Some(API_KEY))
        .unwrap_err()
        .to_string();
    assert!(error.contains("https"));
    let proof = commonmeasure_relay::refresh_directory_proof(
        home.path(),
        API_KEY,
        &json!({"directory_proof": state.lock().unwrap().statement()}),
        std::time::Duration::from_secs(1),
    );
    assert!(
        matches!(proof.action, commonmeasure_relay::ProofAction::NotSent(ref reason) if reason.contains("https"))
    );
    let output = commonmeasure(home.path(), &["policy", "sync"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("https"));
    assert_eq!(state.lock().unwrap().requests, before, "nothing was sent");

    // Disconnect asks nothing, says why, and names both keys to revoke.
    let key_id = commonmeasure_harness::EnrolmentRecord::load(home.path())
        .unwrap()
        .unwrap()
        .key_id;
    let output = commonmeasure(home.path(), &["disconnect"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&format!(
            "edge key {key_id} and its ingest key were not revoked at {cleartext}: the hub was \
             not asked, because the stored hub URL at {cleartext} is neither https nor http to \
             a loopback origin"
        )),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "the edge key {key_id} and the ingest key, each labelled \"laptop-7\""
        )),
        "{stdout}"
    );
    assert!(
        !stdout.contains("run `commonmeasure disconnect`"),
        "{stdout}"
    );
    assert_eq!(state.lock().unwrap().requests, before, "nothing was sent");
    server.stop();
}

#[test]
fn a_stored_cleartext_hub_is_stated_by_status_doctor_and_session_start() {
    let (home, state, mut server, cleartext) = legacy_cleartext_home();
    let before = state.lock().unwrap().requests;

    let output = commonmeasure(home.path(), &["status", "--json"]);
    assert!(output.status.success());
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["egress"]["key_standing"], "cleartext_hub");
    let refused = status["egress"]["hub_refused"].as_str().unwrap();
    assert!(
        refused.contains(&cleartext)
            && refused.contains("relay, standing checks, directory proofs")
            && refused.contains("run `commonmeasure connect` with an https hub"),
        "{refused}"
    );
    for args in [&["status"][..], &["doctor"][..]] {
        let output = commonmeasure(home.path(), args);
        assert!(output.status.success());
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(
            text.contains(&format!("hub URL refused: {refused}")),
            "{args:?}: {text}"
        );
    }

    session_start(home.path(), "s-cleartext");
    let identity = &edge_identity_records(home.path(), "s-cleartext")[0]["payload"];
    assert_eq!(identity["standing"], "cleartext_hub");
    assert_eq!(identity["hub_refused"], refused);
    assert_eq!(state.lock().unwrap().requests, before, "nothing was sent");
    server.stop();
}

/// A home 0.4.1's `connect` enrolled with credentials in a loopback hub URL,
/// which passes the transport rule as an https one does. It wrote the same
/// URL into the receiver and, under `--managed`, into the policy URL. Nothing
/// is sent to the hub, and no output names more than its origin.
#[test]
fn a_stored_hub_url_with_credentials_is_refused_and_named_by_its_origin_alone() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(HubState {
        proof_authority: Some("hub.example"),
        ..HubState::default()
    }));
    let mut server = hub(state.clone());
    let hub_url = server.url().trim_end_matches('/').to_owned();
    assert!(
        commonmeasure(
            home.path(),
            &["connect", &hub_url, "--token", TOKEN, "--managed"]
        )
        .status
        .success()
    );
    let planted = hub_url.replace("http://", "http://ops:ak_PLANTED@");
    for file in ["enrolment.json", "relay.json", "deployment.json"] {
        let path = home.path().join(file);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(&hub_url), "{file}: {text}");
        std::fs::write(&path, text.replace(&hub_url, &planted)).unwrap();
    }
    let key_id = commonmeasure_harness::EnrolmentRecord::load(home.path())
        .unwrap()
        .unwrap()
        .key_id;
    let before = state.lock().unwrap().requests;
    let refused = format!(
        "the enrolled hub URL at {hub_url} carries credentials, a query or a fragment, so \
         nothing is sent to it"
    );

    for args in [&["relay"][..], &["policy", "sync"][..]] {
        let output = commonmeasure(home.path(), args);
        assert!(!output.status.success(), "{args:?}");
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(&refused), "{args:?}: {error}");
        assert!(!error.contains("ak_PLANTED"), "{args:?}: {error}");
    }
    let output = commonmeasure(home.path(), &["status", "--json"]);
    assert!(output.status.success());
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["egress"]["key_standing"], "unusable_hub_url");
    assert!(
        status["egress"]["hub_refused"]
            .as_str()
            .unwrap()
            .starts_with(&refused),
        "{status}"
    );
    for args in [&["status"][..], &["doctor"][..], &["status", "--json"][..]] {
        let output = commonmeasure(home.path(), args);
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(!text.contains("ak_PLANTED"), "{args:?}: {text}");
    }
    let output = commonmeasure(home.path(), &["connect", &hub_url, "--token", TOKEN]);
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains(&format!(
            "already enrolled with the hub at {hub_url} as key {key_id}"
        )),
        "{error}"
    );
    assert!(!error.contains("ak_PLANTED"), "{error}");
    assert_eq!(state.lock().unwrap().requests, before, "nothing was sent");

    let output = commonmeasure(home.path(), &["disconnect"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&format!(
            "edge key {key_id} and its ingest key were not revoked at {hub_url}: the hub was \
             not asked, because the stored hub URL at {hub_url} carries credentials, a query or \
             a fragment\n"
        )),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "deployment.json pinned this hub's policy at {hub_url}; removed"
        )),
        "{stdout}"
    );
    assert!(!stdout.contains("ak_PLANTED"), "{stdout}");
    assert_eq!(state.lock().unwrap().requests, before, "nothing was sent");
    server.stop();
}

/// A 401 for a key other than the enrolled ingest key says nothing about
/// the enrolment: a mistyped `--api-key` must not revoke it.
#[test]
fn a_mistyped_api_key_leaves_the_edge_enrolled_and_signing() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(HubState::default()));
    let mut server = hub(state.clone());
    assert!(
        commonmeasure(home.path(), &["connect", &server.url(), "--token", TOKEN])
            .status
            .success()
    );
    let output = commonmeasure(home.path(), &["relay", "--api-key", "ak_typo"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout
            .contains("standing not checked this run (the hub refused the key this run presented")
            && stdout.contains("given with --api-key"),
        "{stdout}"
    );
    let record = commonmeasure_harness::EnrolmentRecord::load(home.path())
        .unwrap()
        .unwrap();
    assert!(!record.is_revoked(), "{record:?}");
    assert!(
        commonmeasure_harness::identity::Identity::load(home.path())
            .unwrap()
            .signer()
            .is_some()
    );
    let output = commonmeasure(home.path(), &["relay"]);
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(": enrolled"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        state.lock().unwrap().status_keys,
        [
            Some(API_KEY.to_owned()),
            Some("ak_typo".to_owned()),
            Some(API_KEY.to_owned())
        ]
    );
    server.stop();
}

/// A 401 on the enrolled key is recorded as revocation. The hub never
/// answers 200 for a revoked key, so a later 200 for the same key id with no
/// revocation withdraws it, and the edge signs again.
#[test]
fn a_401_revocation_is_withdrawn_when_the_hub_answers_for_the_key_again() {
    const DETAIL: &str = "an auth proxy refused the request";
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(HubState::default()));
    let mut server = hub(state.clone());
    assert!(
        commonmeasure(home.path(), &["connect", &server.url(), "--token", TOKEN])
            .status
            .success()
    );
    state.lock().unwrap().status_error = Some((401, DETAIL));
    let output = commonmeasure(home.path(), &["relay"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&format!(
            "revoked (hub: {DETAIL}): the hub refused the enrolled ingest key"
        )) && !stdout.contains("it is out of the key directory"),
        "{stdout}"
    );
    assert!(
        commonmeasure_harness::identity::Identity::load(home.path())
            .unwrap()
            .signer()
            .is_none()
    );

    state.lock().unwrap().status_error = None;
    let output = commonmeasure(home.path(), &["relay"]);
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(": enrolled"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let record = commonmeasure_harness::EnrolmentRecord::load(home.path())
        .unwrap()
        .unwrap();
    assert!(!record.is_revoked(), "{record:?}");
    assert!(record.revocation.is_none());
    assert!(
        commonmeasure_harness::identity::Identity::load(home.path())
            .unwrap()
            .signer()
            .is_some()
    );
    server.stop();
}

/// A revocation the hub states after a 401 was recorded is kept with the
/// first learnt time, and makes the revocation permanent: a later clean 200,
/// which would withdraw a 401-only revocation, leaves the edge revoked and
/// not signing.
#[test]
fn a_revocation_stated_after_a_401_keeps_the_first_learnt_time() {
    const DETAIL: &str = "the member was removed";
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(HubState::default()));
    let mut server = hub(state.clone());
    assert!(
        commonmeasure(home.path(), &["connect", &server.url(), "--token", TOKEN])
            .status
            .success()
    );
    state.lock().unwrap().status_error = Some((401, DETAIL));
    commonmeasure(home.path(), &["relay"]);
    let first = commonmeasure_harness::EnrolmentRecord::load(home.path())
        .unwrap()
        .unwrap();
    assert!(first.revocation_learnt_at.is_some());
    assert!(first.revoked_at.is_none());

    {
        let mut state = state.lock().unwrap();
        state.status_error = None;
        state.revoked = Some(("2026-09-25T09:00:00Z", "owner"));
    }
    std::thread::sleep(std::time::Duration::from_millis(5));
    let output = commonmeasure(home.path(), &["relay"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("revoked at 2026-09-25T09:00:00Z by the owner"),
        "{stdout}"
    );
    let stated = commonmeasure_harness::EnrolmentRecord::load(home.path())
        .unwrap()
        .unwrap();
    assert_eq!(stated.revocation_learnt_at, first.revocation_learnt_at);
    assert_eq!(stated.revoked_at.as_deref(), Some("2026-09-25T09:00:00Z"));
    assert_eq!(stated.revocation.as_deref(), Some("owner"));

    state.lock().unwrap().revoked = None;
    let output = commonmeasure(home.path(), &["relay"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains(": enrolled"), "{stdout}");
    let after = commonmeasure_harness::EnrolmentRecord::load(home.path())
        .unwrap()
        .unwrap();
    assert_eq!(after, stated);
    assert!(
        commonmeasure_harness::identity::Identity::load(home.path())
            .unwrap()
            .signer()
            .is_none()
    );
    server.stop();
}

/// A proof upload refused with 401 revokes only when the key it was sent
/// under is the enrolled ingest key. Here `relay.json` holds another key
/// than the one presented.
#[test]
fn an_upload_401_for_a_key_other_than_the_enrolled_one_revokes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(HubState {
        proof_authority: Some("hub.example"),
        ..HubState::default()
    }));
    let mut server = hub(state.clone());
    assert!(
        commonmeasure(home.path(), &["connect", &server.url(), "--token", TOKEN])
            .status
            .success()
    );
    let relay = home.path().join("relay.json");
    let mut config: Value = serde_json::from_slice(&std::fs::read(&relay).unwrap()).unwrap();
    config["api_key"] = json!("ak_rotated");
    std::fs::write(&relay, config.to_string()).unwrap();
    let statement = {
        let mut state = state.lock().unwrap();
        state.refuse_upload = Some((401, "a valid credential is required"));
        state.held = None;
        state.statement()
    };
    let uploads = state.lock().unwrap().uploads.len();
    let proof = commonmeasure_relay::refresh_directory_proof(
        home.path(),
        API_KEY,
        &json!({"directory_proof": statement}),
        std::time::Duration::from_secs(5),
    );
    assert!(
        matches!(proof.action, commonmeasure_relay::ProofAction::Failed(_)),
        "{:?}",
        proof.action
    );
    assert_eq!(state.lock().unwrap().uploads.len(), uploads + 1);
    assert!(
        !commonmeasure_harness::EnrolmentRecord::load(home.path())
            .unwrap()
            .unwrap()
            .is_revoked()
    );
    server.stop();
}

/// `disconnect` removes the enrolment record under the record's lock, so a
/// process that read the record earlier and is writing a revocation back
/// cannot bring it back afterwards. Here the test holds the lock as such a
/// writer would: the record stays until it is released.
#[test]
fn disconnect_waits_for_a_writer_holding_the_enrolment_record() {
    let home = tempfile::tempdir().unwrap();
    let state = Arc::new(Mutex::new(HubState::default()));
    let mut server = hub(state);
    assert!(
        commonmeasure(home.path(), &["connect", &server.url(), "--token", TOKEN])
            .status
            .success()
    );
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.path().join("enrolment.lock"))
        .unwrap();
    lock.lock().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .arg("disconnect")
        .env("COMMONMEASURE_HOME", home.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // `relay.json` goes just before the record; once it has gone,
    // `disconnect` is at the record.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while home.path().join("relay.json").exists() {
        assert!(std::time::Instant::now() < deadline, "disconnect never ran");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(child.try_wait().unwrap().is_none());
    assert!(
        home.path().join("enrolment.json").exists(),
        "disconnect removed the record while another writer held it"
    );
    drop(lock);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!home.path().join("enrolment.json").exists());
    server.stop();
}
