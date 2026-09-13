//! Enrolment driven as an operator does it: the real binary connecting to a
//! hub, recording sessions, relaying, learning a revocation and
//! disconnecting. The offline tests use a loopback `commonmeasure_http::Server`
//! that answers the hub's enrolment routes and verifies the proof of
//! possession the binary sends, so the bytes on the wire are the ones a
//! real hub receives. Its answers copy the shapes of the hub's handlers in
//! the hub repository: `crates/server/src/enrolment.rs`
//! (exchange, status, disconnect) and `crates/server/src/telemetry.rs`
//! (the acceptance body); a change to either shape is a change here. The
//! one test against a running hub is ignored by default; see
//! `a_real_hub_enrols_revokes_and_disconnects_this_edge` for how to run it.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use commonmeasure_http::{Request, Response, Server, ServerHandle, send};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const TOKEN: &str = "et_0123456789abcdef";
const API_KEY: &str = "ak_hub_issued_secret";
/// The origin this hub says it publishes the enrolled key under. The edge
/// stores it and names it in `Signature-Agent` on every signed request; it
/// never invents an origin of its own.
const ORIGIN: &str = "https://hub.example";

/// What the loopback hub saw and how it answers.
#[derive(Default)]
struct HubState {
    /// The exchange request body, verbatim.
    exchange_bodies: Vec<Value>,
    /// The key id the hub assigned (the thumbprint of the registered key).
    key_id: Option<String>,
    /// Batches delivered to the telemetry receiver.
    batches: Vec<Value>,
    /// `X-API-Key` values seen on status and disconnect calls.
    status_keys: Vec<Option<String>>,
    disconnect_keys: Vec<Option<String>>,
    /// When set, the status route answers a revocation.
    revoked: Option<(&'static str, &'static str)>,
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
                    Response::json(
                        201,
                        &json!({
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
                            "telemetry_path": "/api/v1/telemetry",
                        })
                        .to_string(),
                    )
                }
                ("GET", "/api/v1/enrolment/status") => {
                    state.status_keys.push(api_key.clone());
                    if api_key.as_deref() != Some(API_KEY) {
                        return Response::json(401, r#"{"detail":"a valid credential is required"}"#);
                    }
                    let (revoked_at, revocation) = match state.revoked {
                        Some((at, by)) => (json!(at), json!(by)),
                        None => (Value::Null, Value::Null),
                    };
                    Response::json(
                        200,
                        &json!({
                            "key_id": state.key_id,
                            "name": "laptop-7",
                            "revoked_at": revoked_at,
                            "revocation": revocation,
                        })
                        .to_string(),
                    )
                }
                ("POST", "/api/v1/enrolment/disconnect") => {
                    state.disconnect_keys.push(api_key.clone());
                    if api_key.as_deref() != Some(API_KEY) {
                        return Response::json(401, r#"{"detail":"a valid credential is required"}"#);
                    }
                    Response::new(204, Vec::new())
                }
                ("POST", "/api/v1/telemetry/events") => {
                    if api_key.as_deref() != Some(API_KEY) {
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
        stdout.contains(&format!("edge key {key_id} was not revoked at {hub_url}")),
        "{stdout}"
    );
    assert!(stdout.contains("revoke it from the hub"), "{stdout}");
    for file in ["relay.json", "edge-key.json", "enrolment.json"] {
        assert!(
            !home.path().join(file).exists(),
            "{file} survived disconnect"
        );
    }
}

/// A token the hub refuses leaves nothing behind: no key, no record, no
/// relay configuration.
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

/// The whole enrolment life against a running Common Measure Hub, driven
/// by this binary: the real exchange, the key in the hub's directory,
/// revocation from the hub's API, the next relay run learning it, and
/// disconnect. Ignored by default because it needs a hub. To run it:
///
/// 1. In a checkout of the hub repository, start Postgres, apply migrations
///    and run the server with `IDENTITY_ORIGIN` set (its README's
///    quickstart), with an organisation whose owner has a session; the
///    hub's `crates/server/tests/fixtures/setup.sql` is one way to seed
///    one.
/// 2. Export `COMMONMEASURE_TEST_HUB` (the server's base URL, e.g.
///    `http://127.0.0.1:8080`) and `COMMONMEASURE_TEST_HUB_SESSION` (the
///    owner's raw session token, sent as the `__Host-session` cookie).
/// 3. `cargo test -p commonmeasure-cli --test connect_e2e -- --ignored --nocapture`
///
/// It prints a transcript with every secret redacted;
/// `demo/enrolment/real-hub-run.txt` is one such run.
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
    let directory = || -> Value {
        let response = send(
            &format!("{hub_url}/.well-known/http-message-signatures-directory"),
            Request::get("/"),
        )
        .expect("the hub answers");
        assert_eq!(response.status, 200);
        assert!(
            response.headers.get("Signature-Input").is_some(),
            "the directory is signed"
        );
        serde_json::from_slice(&response.body).expect("json")
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

    println!("\n## 4. The key is in the hub's signed directory and in the owner's listing");
    let listed = directory();
    assert!(kids(&listed).contains(&key_id));
    println!("directory kids: {:?}", kids(&listed));
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
    let after = directory();
    assert!(!kids(&after).contains(&key_id));
    println!("directory kids: {:?}", kids(&after));
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
