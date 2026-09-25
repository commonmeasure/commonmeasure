//! The hosted edge in service mode, driven as the deployed process is: the
//! real binary under `hosted service`, a loopback hub standing for the
//! policy endpoint, the authorisation server and the telemetry receiver, and
//! the relay, the policy refresh and the key refresh running on the service's
//! own interval with no command run.
//!
//! Nothing of the product is substituted. The hub is a loopback server
//! because the hub is not here; it verifies the edge's Web Bot Auth signature
//! on the policy request as the hub does, serves envelopes signed with a real
//! Ed25519 key, publishes a JWKS and signs access tokens under it. Its
//! supplier credential release route is a test double written from
//! `docs/contracts/supplier-credentials.md` §Release: it verifies the same
//! signature, requires it to cover `@method` and `@path`, holds the window
//! to 360 seconds and the nonce to at least 22 characters, refuses a repeated
//! nonce and serves what the test set. It establishes nothing about the hub's
//! own route.

use std::io::{BufRead, BufReader, Write as _};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use commonmeasure_harness::identity::EdgeKey;
use commonmeasure_harness::managed::{ENVELOPE, encode_hex};
use commonmeasure_harness::policy::PolicyFile;
use commonmeasure_http::{Request, Response, Server, ServerHandle, send};
use commonmeasure_types::canonical::{canonical_digest, canonical_json};
use ring::signature::KeyPair;
use serde_json::{Value, json};

mod credential_sweep;
#[path = "hosted_service/reporting.rs"]
mod reporting;
mod webbotauth;

const ORIGIN: &str = "https://edge.test";
/// The organisation `webbotauth::enrol` writes into the enrolment record and
/// the envelopes and tokens must name.
const ORGANISATION: &str = "org-1";
const KEY_ID: &str = "hub-key-1";
const SIGNER_KEY_ID: &str = "hub-policy-1";

/// The managed policy the hub distributes: it admits private addresses in
/// both the ways a policy can, which the service must decline, and clears
/// one engagement's crossings to leave, which the relay must deliver.
fn policy() -> Value {
    json!({
        "policy_mode": "observe",
        "allow_private_hosts": true,
        "record_internal_prefixes": ["http://169.254.169.254/", "http://metadata.google.internal/"],
        "scopes": [{"match": "/work/acme", "engagement": "acme", "allow_telemetry_egress": true}]
    })
}

/// The hub as the service sees it, on one loopback origin: the policy
/// endpoint (`/api/v1/policy/desired`, admitting a signed request from the
/// enrolled key), the authorisation-server metadata and JWKS, and the
/// telemetry receiver the relay delivers to. Counts what each is asked.
struct Hub {
    handle: ServerHandle,
    policy_signer: ring::signature::Ed25519KeyPair,
    signer_public_hex: String,
    token_key: EdgeKey,
    directory: Arc<Mutex<webbotauth::Directory>>,
    policy_requests: Arc<AtomicUsize>,
    jwks_reads: Arc<AtomicUsize>,
    batches: Arc<Mutex<Vec<Value>>>,
    /// The `credentials` list the release route serves the enrolled edge.
    released: Arc<Mutex<Vec<Value>>>,
    release_requests: Arc<AtomicUsize>,
    /// Set, the release route answers `503` and rules on nothing: an
    /// injected fault standing for a hub outage.
    release_outage: Arc<AtomicBool>,
    /// Set, the telemetry receiver answers `503` and records nothing: an
    /// injected fault that leaves a batch queued on the edge.
    delivery_outage: Arc<AtomicBool>,
    /// The signed reporting-approval snapshot the approvals route serves the
    /// enrolled edge; unset, the route answers `404`.
    approvals: Arc<Mutex<Option<Value>>>,
}

impl Hub {
    fn start() -> Self {
        Self::with_oauth(true)
    }

    fn with_oauth(oauth_available: bool) -> Self {
        let pair = {
            let rng = ring::rand::SystemRandom::new();
            let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).expect("a key pair");
            ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("pkcs8")
        };
        let signer_public_hex = encode_hex(pair.public_key().as_ref());
        let envelope = envelope(&pair, 1, policy());
        let token_key = EdgeKey::generate().expect("a key");
        let jwk_x = token_key.jwk_x();
        let directory = Arc::new(Mutex::new(webbotauth::Directory::default()));
        let policy_requests = Arc::new(AtomicUsize::new(0));
        let jwks_reads = Arc::new(AtomicUsize::new(0));
        let batches = Arc::new(Mutex::new(Vec::new()));
        let released = Arc::new(Mutex::new(Vec::<Value>::new()));
        let release_requests = Arc::new(AtomicUsize::new(0));
        let release_outage = Arc::new(AtomicBool::new(false));
        let delivery_outage = Arc::new(AtomicBool::new(false));
        let approvals = Arc::new(Mutex::new(None::<Value>));
        let release_nonces = Mutex::new(std::collections::HashSet::new());
        let listener = Server::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        let handle = listener
            .spawn({
                let (policy_requests, jwks_reads, batches, directory) = (
                    Arc::clone(&policy_requests),
                    Arc::clone(&jwks_reads),
                    Arc::clone(&batches),
                    Arc::clone(&directory),
                );
                let (released, release_requests, release_outage, delivery_outage) = (
                    Arc::clone(&released),
                    Arc::clone(&release_requests),
                    Arc::clone(&release_outage),
                    Arc::clone(&delivery_outage),
                );
                let served_approvals = Arc::clone(&approvals);
                let url = url.clone();
                move |request| match request.target.as_str() {
                    "/api/v1/edge/reporting-approvals" => {
                        match served_approvals.lock().expect("lock").as_ref() {
                            Some(snapshot) => Response::json(200, &snapshot.to_string()),
                            None => Response::json(404, r#"{"detail":"not found"}"#),
                        }
                    }
                    "/api/v1/policy/desired" => {
                        policy_requests.fetch_add(1, Ordering::SeqCst);
                        let authority = request.headers.get("Host").unwrap_or_default().to_owned();
                        match webbotauth::verify(
                            &request,
                            &authority,
                            &directory.lock().expect("lock"),
                        ) {
                            Ok(_) => {
                                let mut response = Response::new(200, envelope.clone());
                                response.headers.set("Content-Type", "application/json");
                                response
                            }
                            Err(reason) => Response::json(
                                401,
                                &json!({"detail": format!("not signed by an enrolled key: {reason}")})
                                    .to_string(),
                            ),
                        }
                    }
                    "/api/v1/supplier-credentials" if release_outage.load(Ordering::SeqCst) => {
                        Response::json(503, r#"{"detail":"unavailable"}"#)
                    }
                    "/api/v1/supplier-credentials" => {
                        release_requests.fetch_add(1, Ordering::SeqCst);
                        let authority = request.headers.get("Host").unwrap_or_default().to_owned();
                        let verified = webbotauth::verify_release(
                            &request,
                            &authority,
                            &directory.lock().expect("lock"),
                        )
                        .and_then(|verified| {
                            // The route serves a secret, so a signature is
                            // good once.
                            let input = request
                                .headers
                                .get("Signature-Input")
                                .unwrap_or_default()
                                .to_owned();
                            if request.headers.get("Authorization").is_some() {
                                Err("an API key does not authenticate this route".to_owned())
                            } else if release_nonces.lock().expect("lock").insert(input) {
                                Ok(verified)
                            } else {
                                Err("the nonce was seen before".to_owned())
                            }
                        });
                        match verified {
                            Ok(verified) => {
                                let mut response = Response::json(
                                    200,
                                    &json!({
                                        "organisation": ORGANISATION,
                                        "key_id": verified.key_id,
                                        "served_at": chrono::Utc::now().to_rfc3339(),
                                        "credentials": *released.lock().expect("lock"),
                                    })
                                    .to_string(),
                                );
                                response.headers.set("Cache-Control", "no-store");
                                response
                            }
                            Err(reason) => Response::json(
                                401,
                                &json!({"detail": format!("not signed by an enrolled key: {reason}")})
                                    .to_string(),
                            ),
                        }
                    }
                    "/.well-known/oauth-authorization-server" | "/.well-known/jwks.json"
                        if !oauth_available =>
                    {
                        Response::json(404, r#"{"detail":"authorisation server not built"}"#)
                    }
                    "/.well-known/oauth-authorization-server" => Response::json(
                        200,
                        &json!({
                            "issuer": url,
                            "jwks_uri": format!("{url}/.well-known/jwks.json"),
                            "authorization_endpoint": format!("{url}/oauth/authorize"),
                            "token_endpoint": format!("{url}/api/v1/oauth/token"),
                            "code_challenge_methods_supported": ["S256"],
                        })
                        .to_string(),
                    ),
                    "/.well-known/jwks.json" => {
                        jwks_reads.fetch_add(1, Ordering::SeqCst);
                        Response::json(
                            200,
                            &json!({"keys": [
                                {"kty": "OKP", "crv": "Ed25519", "kid": KEY_ID, "x": jwk_x, "use": "sig", "alg": "EdDSA"}
                            ]})
                            .to_string(),
                        )
                    }
                    target
                        if request.method == "POST"
                            && target.contains("events")
                            && delivery_outage.load(Ordering::SeqCst) =>
                    {
                        Response::json(503, r#"{"detail":"unavailable"}"#)
                    }
                    target if request.method == "POST" && target.contains("events") => {
                        let body: Value =
                            serde_json::from_slice(&request.body).unwrap_or(Value::Null);
                        let events = body["events"].as_array().map(Vec::len).unwrap_or(0);
                        batches.lock().expect("lock").push(body);
                        Response::json(
                            201,
                            &json!({"status": "ok", "events_created": events}).to_string(),
                        )
                    }
                    _ => Response::json(404, r#"{"detail":"not found"}"#),
                }
            })
            .expect("spawn");
        Self {
            handle,
            policy_signer: pair,
            signer_public_hex,
            token_key,
            directory,
            policy_requests,
            jwks_reads,
            batches,
            released,
            release_requests,
            release_outage,
            delivery_outage,
            approvals,
        }
    }

    fn url(&self) -> String {
        self.handle.url()
    }

    /// An operator home enrolled with this hub under managed policy, as
    /// `connect --managed` leaves it: the edge key, the enrolment record,
    /// `deployment.json` pinning the policy signer, and `relay.json` naming
    /// this hub as the receiver.
    fn enrol_managed(&self, home: &Path) {
        webbotauth::enrol(
            home,
            &self.url(),
            "https://hub.example",
            &mut self.directory.lock().expect("lock"),
        );
        std::fs::write(
            home.join("deployment.json"),
            json!({
                "mode": "managed",
                "signer": {"key_id": SIGNER_KEY_ID, "algorithm": "ed25519",
                           "public_key": self.signer_public_hex},
                "policy_url": format!("{}/api/v1/policy/desired", self.url()),
                "organisation": ORGANISATION,
            })
            .to_string(),
        )
        .expect("deployment");
        std::fs::write(
            home.join("relay.json"),
            json!({"receiver": self.url(), "api_key": "ingest-key-1"}).to_string(),
        )
        .expect("relay configuration");
    }

    fn bearer(&self, subject: &str, endpoint: &str) -> String {
        let now = chrono::Utc::now().timestamp();
        let claims = json!({
            "iss": self.url(),
            "sub": subject,
            "aud": format!("{ORIGIN}/mcp/{endpoint}"),
            "org": ORGANISATION,
            "client_id": "https://claude.ai/.well-known/claude-client-metadata.json",
            "scope": "",
            "iat": now,
            "exp": now + 3600,
            "jti": "grant-1",
        });
        let header = json!({"alg": "EdDSA", "typ": "at+jwt", "kid": KEY_ID});
        let signed = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        let signature = self.token_key.sign(signed.as_bytes());
        format!("Bearer {signed}.{signature}")
    }
}

/// A signed envelope carrying `policy` at `revision`, as the hub's signer
/// publishes it.
fn envelope(pair: &ring::signature::Ed25519KeyPair, revision: u64, policy: Value) -> Vec<u8> {
    let carried = serde_json::from_value::<PolicyFile>(policy.clone())
        .map(|policy| serde_json::to_value(&policy).expect("serialises"))
        .unwrap_or(policy);
    let payload = json!({
        "organisation": ORGANISATION,
        "edge_key_id": null,
        "revision": revision,
        "issued_at": "2026-09-15T00:00:00Z",
        "expires_at": "2099-01-01T00:00:00Z",
        "policy": carried,
        "digest": canonical_digest(&carried),
    });
    let signature = pair.sign(canonical_json(&payload).as_bytes());
    serde_json::to_vec(&json!({
        "envelope": ENVELOPE,
        "payload": payload,
        "signature": {
            "algorithm": "ed25519",
            "key_id": SIGNER_KEY_ID,
            "value": encode_hex(signature.as_ref()),
        },
    }))
    .expect("serialises")
}

fn write_service_config(home: &Path, interval_seconds: u64) {
    std::fs::write(
        home.join("hosted-service.json"),
        json!({
            "listen": "127.0.0.1:0",
            "origin": ORIGIN,
            "hosts": ["claude-connector", "copilot-cloud-agent"],
            "interval_seconds": interval_seconds,
        })
        .to_string(),
    )
    .expect("service configuration");
}

fn service_command(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_commonmeasure"));
    command
        .args(["hosted", "service"])
        .env("COMMONMEASURE_HOME", home)
        .env("HOME", home);
    command
}

/// The real binary as the service, on an OS-chosen loopback port.
struct Service {
    child: Child,
    base: String,
}

impl Service {
    fn start(home: &Path) -> Self {
        Self::start_with(service_command(home), Stdio::null())
    }

    /// The service started from `command`, its journal sent to `journal`.
    fn start_with(mut command: Command, journal: Stdio) -> Self {
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(journal)
            .spawn()
            .expect("the binary should start");
        let stdout = child.stdout.as_mut().expect("stdout");
        let mut lines = BufReader::new(stdout).lines();
        let base = loop {
            let line = lines
                .next()
                .expect("hosted service announces its address before serving")
                .expect("readable stdout");
            if let Some(address) = line.strip_prefix("listening on ") {
                break address.trim().to_owned();
            }
        };
        Self { child, base }
    }

    fn post(&self, path: &str, headers: &[(&str, &str)], body: &Value) -> Response {
        let mut request = Request::post(path, body.to_string().into_bytes(), "application/json");
        request
            .headers
            .set("Accept", "application/json, text/event-stream");
        for (name, value) in headers {
            request.headers.set(name, value);
        }
        send(&format!("{}{path}", self.base), request).expect("the request completes")
    }

    fn open_session(&self, endpoint: &str, authorization: &str) -> String {
        let response = self.post(
            &format!("/mcp/{endpoint}"),
            &[("Authorization", authorization)],
            &json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
                    "params": {"protocolVersion": "2025-11-25", "capabilities": {},
                               "clientInfo": {"name": "claude-ai", "version": "1"}}}),
        );
        assert_eq!(response.status, 200, "{}", text(&response));
        assert_eq!(body(&response)["result"]["protocolVersion"], "2025-11-25");
        response
            .headers
            .get("Mcp-Session-Id")
            .expect("a session id")
            .to_owned()
    }

    fn call(
        &self,
        endpoint: &str,
        authorization: &str,
        session: &str,
        id: u64,
        name: &str,
        arguments: Value,
    ) -> Response {
        self.post(
            &format!("/mcp/{endpoint}"),
            &[
                ("Authorization", authorization),
                ("Mcp-Session-Id", session),
                ("MCP-Protocol-Version", "2025-11-25"),
            ],
            &json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
                    "params": {"name": name, "arguments": arguments}}),
        )
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn body(response: &Response) -> Value {
    serde_json::from_slice(&response.body).unwrap_or_else(|_| panic!("JSON: {}", text(response)))
}

fn text(response: &Response) -> String {
    format!(
        "{} {}",
        response.status,
        String::from_utf8_lossy(&response.body)
    )
}

/// The tool result's payload, which the server delivers as JSON text.
fn payload(response: &Response) -> Value {
    let body = body(response);
    let text = body["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a tool result: {body}"));
    serde_json::from_str(text).expect("the payload is JSON")
}

fn run(home: &Path, args: &[&str]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(args)
        .env("COMMONMEASURE_HOME", home)
        .env("HOME", home)
        .output()
        .expect("the binary runs");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// One witnessed crossing recorded through the observed path in a directory
/// the managed policy clears, so the relay has something to deliver.
fn record_cleared_crossing(home: &Path, session: &str) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["hook", "post-tool-use"])
        .env("COMMONMEASURE_HOME", home)
        .stdin(Stdio::piped())
        .spawn()
        .expect("hook should start");
    writeln!(
        child.stdin.as_mut().expect("stdin"),
        "{}",
        json!({
            "session_id": session,
            "cwd": "/work/acme/site",
            "hook_event_name": "PostToolUse",
            "tool_name": "WebFetch",
            "tool_input": {"url": "https://www.example.com/pricing"},
            "tool_response": {"result": "Enough page text to count as grounded."}
        })
    )
    .expect("write");
    assert!(child.wait().expect("wait").success());
}

fn wait_until(what: &str, budget: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + budget;
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "{what} did not happen within {}s",
            budget.as_secs()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn the_service_refuses_to_start_unenrolled_unmanaged_unconfigured_or_on_a_locked_home() {
    let hub = Hub::start();
    let home = tempfile::tempdir().expect("tempdir");

    // No configuration: the first thing an operator writes.
    let output = service_command(home.path()).output().expect("runs");
    assert!(!output.status.success());
    let reason = String::from_utf8_lossy(&output.stderr);
    assert!(reason.contains("hosted-service.json"), "{reason}");

    // Unenrolled: no issuer to trust, no key to sign with.
    write_service_config(home.path(), 300);
    let output = service_command(home.path()).output().expect("runs");
    assert!(!output.status.success());
    let reason = String::from_utf8_lossy(&output.stderr);
    assert!(reason.contains("enrolment record"), "{reason}");

    // Enrolled but local: the policy would be a file on this machine.
    webbotauth::enrol(
        home.path(),
        &hub.url(),
        "https://hub.example",
        &mut hub.directory.lock().expect("lock"),
    );
    let output = service_command(home.path()).output().expect("runs");
    assert!(!output.status.success());
    let reason = String::from_utf8_lossy(&output.stderr);
    assert!(
        reason.contains("managed policy") && reason.contains("deployment.json"),
        "{reason}"
    );

    // Managed: starts. A second process on the same home is refused while
    // the first runs, and starts once it has gone.
    hub.enrol_managed(home.path());
    let first = Service::start(home.path());
    let output = service_command(home.path()).output().expect("runs");
    assert!(!output.status.success());
    let reason = String::from_utf8_lossy(&output.stderr);
    assert!(
        reason.contains("another hosted service holds") && reason.contains("hosted-service.lock"),
        "{reason}"
    );
    let (ok, stdout, _) = run(home.path(), &["doctor"]);
    assert!(ok, "{stdout}");
    assert!(
        stdout.contains("hosted service: running (lock held) at https://edge.test"),
        "{stdout}"
    );
    assert!(
        stdout.contains("/mcp/claude-connector /mcp/copilot-cloud-agent"),
        "{stdout}"
    );
    let (ok, stdout, _) = run(home.path(), &["status"]);
    assert!(ok, "{stdout}");
    assert!(
        stdout.contains("service mode      running (lock held) at https://edge.test"),
        "{stdout}"
    );
    drop(first);
    let (ok, stdout, _) = run(home.path(), &["status"]);
    assert!(ok, "{stdout}");
    assert!(
        stdout.contains("service mode      configured, not running at https://edge.test"),
        "{stdout}"
    );
    let second = Service::start(home.path());
    drop(second);
}

#[test]
fn the_service_holds_the_private_address_floor_whatever_the_managed_policy_says() {
    let hub = Hub::start();
    let home = tempfile::tempdir().expect("tempdir");
    hub.enrol_managed(home.path());
    write_service_config(home.path(), 300);
    let service = Service::start(home.path());
    let authorization = hub.bearer("user-1", "claude-connector");
    let session = service.open_session("claude-connector", &authorization);

    // The managed policy arrived at session start and admits private
    // addresses both ways; the status says the floor is held over it.
    let status = service.call(
        "claude-connector",
        &authorization,
        &session,
        1,
        "context_status",
        json!({}),
    );
    let reported = payload(&status);
    assert_eq!(
        reported["policy"]["allow_private_hosts"], true,
        "{reported}"
    );
    assert_eq!(reported["policy"]["private_floor"], "held", "{reported}");
    assert!(reported["cwd"].is_null(), "a hosted session has no cwd");
    assert!(
        hub.policy_requests.load(Ordering::SeqCst) >= 1,
        "the session start refreshed the managed policy"
    );

    for (id, url) in [
        (
            2,
            "http://169.254.169.254/computeMetadata/v1/instance/service-accounts/",
        ),
        (3, "http://metadata.google.internal/computeMetadata/v1/"),
        (4, "http://10.128.0.7/"),
        (5, "http://192.168.1.4/"),
        (6, "http://127.0.0.1:8080/"),
    ] {
        let refused = service.call(
            "claude-connector",
            &authorization,
            &session,
            id,
            "context_fetch",
            json!({"url": url}),
        );
        assert_eq!(refused.status, 200, "{}", text(&refused));
        let result = body(&refused)["result"].clone();
        assert_eq!(result["isError"], true, "{url}: {result}");
        let detail = result["content"][0]["text"].as_str().unwrap_or_default();
        assert!(
            detail.contains("does not mediate local or private addresses")
                && detail.contains("service mode"),
            "{url}: {detail}"
        );
    }
    let records = commonmeasure_harness::SessionLog::read(
        &home
            .path()
            .join("sessions")
            .join(format!("{session}.ndjson")),
    )
    .expect("the session file reads");
    assert!(
        records.iter().all(|record| !record["event"]
            .as_str()
            .unwrap_or_default()
            .starts_with("crossing_")),
        "an address outside the floor is not a crossing and leaves no crossing record: {records:?}"
    );
}

#[test]
fn the_service_relays_refreshes_policy_and_keys_and_sweeps_on_its_interval_with_no_command_run() {
    let hub = Hub::start();
    let home = tempfile::tempdir().expect("tempdir");
    hub.enrol_managed(home.path());
    write_service_config(home.path(), 1);
    record_cleared_crossing(home.path(), "acme-session-1");
    let service = Service::start(home.path());

    // The relay ran on the interval: the cleared crossing reached the
    // receiver, and the policy was refreshed on the way (the relay's own
    // refresh, then the interval's).
    wait_until("the relay's delivery", Duration::from_secs(15), || {
        !hub.batches.lock().expect("lock").is_empty()
    });
    let batches = hub.batches.lock().expect("lock").clone();
    let events = batches[0]["events"].as_array().expect("events");
    assert!(
        events
            .iter()
            .any(|event| event.to_string().contains("www.example.com")),
        "{batches:?}"
    );
    assert!(hub.policy_requests.load(Ordering::SeqCst) >= 1);
    assert_eq!(
        std::fs::read_to_string(home.path().join("policy.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .map(|policy| policy["policy_mode"].clone()),
        Some(json!("observe")),
        "the distributed policy is on disk"
    );

    // The issuer's keys are refetched on the interval, not only for an
    // unknown key id, so a withdrawn key stops verifying within one.
    let reads = hub.jwks_reads.load(Ordering::SeqCst);
    wait_until("the key refresh", Duration::from_secs(10), || {
        hub.jwks_reads.load(Ordering::SeqCst) > reads
    });
    assert!(
        home.path().join("hosted-jwks.json").exists(),
        "the refreshed keys are cached on disk"
    );

    // A token verifies against the refreshed cache with no further read.
    let before = hub.jwks_reads.load(Ordering::SeqCst);
    let authorization = hub.bearer("user-2", "copilot-cloud-agent");
    let session = service.open_session("copilot-cloud-agent", &authorization);
    let status = service.call(
        "copilot-cloud-agent",
        &authorization,
        &session,
        1,
        "context_status",
        json!({}),
    );
    assert_eq!(status.status, 200, "{}", text(&status));
    assert!(
        hub.jwks_reads.load(Ordering::SeqCst) - before <= 1,
        "a verification costs at most the interval's own read"
    );
    drop(service);
    let (ok, stdout, _) = run(home.path(), &["status"]);
    assert!(ok, "{stdout}");
    assert!(stdout.contains("every 1s"), "{stdout}");
}

/// An operator who writes `relay/manual` reviews each run before it leaves:
/// the interval relay does not run while the marker is there, the journal
/// says why on every interval, and removing the marker restores delivery
/// with no restart.
#[test]
fn the_service_skips_its_interval_relay_while_the_manual_marker_is_present() {
    let hub = Hub::start();
    let home = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    hub.enrol_managed(home.path());
    write_service_config(home.path(), 1);
    record_cleared_crossing(home.path(), "acme-session-1");
    let marker = home.path().join("relay").join("manual");
    std::fs::create_dir_all(marker.parent().expect("relay directory")).expect("relay directory");
    std::fs::write(&marker, b"").expect("marker");
    let journal_path = elsewhere.path().join("journal.txt");
    let service = Service::start_with(
        service_command(home.path()),
        Stdio::from(std::fs::File::create(&journal_path).expect("journal")),
    );
    let journal = || std::fs::read_to_string(&journal_path).unwrap_or_default();

    // Two intervals, each skipping the relay and naming the marker; the
    // rest of the interval work still runs.
    wait_until("two skipped relays", Duration::from_secs(15), || {
        journal().matches("relay skipped").count() >= 2
    });
    let skipped = journal();
    assert!(
        skipped
            .lines()
            .any(|line| line.contains("relay skipped")
                && line.contains(&marker.display().to_string())),
        "{skipped}"
    );
    assert!(!skipped.contains("relay to "), "{skipped}");
    assert!(skipped.contains("issuer keys refreshed"), "{skipped}");
    assert!(
        hub.batches.lock().expect("lock").is_empty(),
        "nothing leaves while the marker is there"
    );

    std::fs::remove_file(&marker).expect("remove the marker");
    wait_until("the relay's delivery", Duration::from_secs(15), || {
        !hub.batches.lock().expect("lock").is_empty()
    });
    drop(service);
}

/// Establishes service behaviour with missing OAuth endpoints, not Microsoft
/// interoperability. The managed-policy endpoint remains available.
#[test]
fn an_m365_edge_token_works_without_oauth_and_keeps_the_service_floor() {
    let hub = Hub::with_oauth(false);
    let home = tempfile::tempdir().expect("tempdir");
    hub.enrol_managed(home.path());
    std::fs::write(
        home.path().join("hosted-service.json"),
        json!({"listen": "127.0.0.1:0", "origin": ORIGIN,
               "hosts": ["m365-copilot"], "interval_seconds": 1})
        .to_string(),
    )
    .expect("service configuration");
    let (ok, token, error) = run(
        home.path(),
        &[
            "hosted",
            "token",
            "issue",
            "m365:operator",
            "--host",
            "m365-copilot",
        ],
    );
    assert!(ok, "{error}");
    let authorization = format!("Bearer {}", token.trim());
    let service = Service::start(home.path());
    let response = service.post(
        "/mcp/m365-copilot",
        &[("Authorization", &authorization)],
        &json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "m365-header-fixture", "version": "1"}}}),
    );
    assert_eq!(response.status, 200, "{}", text(&response));
    assert_eq!(body(&response)["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(
        response.headers.get("Content-Type"),
        Some("application/json")
    );
    let session = response.headers.get("Mcp-Session-Id").expect("session");
    let headers = [
        ("Authorization", authorization.as_str()),
        ("Mcp-Session-Id", session),
        ("MCP-Protocol-Version", "2025-06-18"),
    ];
    let status_request = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "context_status", "arguments": {}}});
    let status = service.post("/mcp/m365-copilot", &headers, &status_request);
    assert_eq!(payload(&status)["policy"]["private_floor"], "held");
    assert_eq!(payload(&status)["host"], "m365-copilot");
    wait_until(
        "the managed policy interval",
        Duration::from_secs(10),
        || hub.policy_requests.load(Ordering::SeqCst) >= 2,
    );
    let refused = service.post(
        "/mcp/m365-copilot",
        &headers,
        &json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "context_fetch", "arguments": {"url": "http://169.254.169.254/"}}
        }),
    );
    assert_eq!(refused.status, 200);
    assert_eq!(body(&refused)["result"]["isError"], true);
    assert!(text(&refused).contains("service mode"));
    assert_eq!(hub.jwks_reads.load(Ordering::SeqCst), 0);
    assert!(!home.path().join("hosted-jwks.json").exists());
    let records = commonmeasure_harness::SessionLog::read(
        &home
            .path()
            .join("sessions")
            .join(format!("{session}.ndjson")),
    )
    .expect("session records");
    assert!(records.iter().all(|r| r["event"] != "crossing_mediated"));
    let (ok, _, error) = run(home.path(), &["hosted", "token", "revoke", "m365:operator"]);
    assert!(ok, "{error}");
    let refused = service.post("/mcp/m365-copilot", &headers, &status_request);
    assert_eq!(refused.status, 401);
    assert!(text(&refused).contains("revoked"));
}

/// Every file under `root` as text, with the path before each.
fn everything_under(root: &Path) -> String {
    let mut text = String::new();
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("readable") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                text.push_str(&format!("\n== {} ==\n", path.display()));
                text.push_str(&String::from_utf8_lossy(
                    &std::fs::read(&path).expect("read"),
                ));
            }
        }
    }
    text
}

/// The supplier credential custody path through the real binary
/// (`docs/contracts/supplier-credentials.md` §Edge side and §Revocation and
/// rotation), against this file's **test double** of the release route. It
/// establishes what the service does with a release, a rotation and a
/// revocation, and that a released value is in no session record, fetch
/// record, spool file, journal line or tool answer. It does not establish
/// integration with the hub, and no supplier is called: a search is made
/// only once no key is held, where it fails before any request.
// Catches: a service that fetches without being configured to; a release not
// named in `context_status` or the source record; a rotation or revocation
// that needs a restart; a value written anywhere on the machine or said to
// the agent; the unavailable message mentioning the hub.
#[test]
fn a_hub_released_key_is_used_by_name_rotated_and_revoked_with_no_restart_and_is_written_nowhere() {
    const FIRST: &str = "exa-released-41c7e0aa-first";
    const ROTATED: &str = "exa-released-41c7e0aa-rotated";
    // A second release in a shape the product's sweep recognises. The Exa
    // values above match none of its markers, so without this the sweep at
    // the end could not see a released value at all.
    const TAVILY: &str = "tvly-released-41c7e0aa";
    let entry = |value: &str, rotated_at: Option<&str>| {
        json!({"connection_id": "conn-exa-1", "provider": "exa", "variable": "EXA_API_KEY",
               "value": value, "rotated_at": rotated_at})
    };
    let tavily = || {
        json!({"connection_id": "conn-tavily-1", "provider": "tavily",
               "variable": "TAVILY_API_KEY", "value": TAVILY, "rotated_at": null})
    };

    let hub = Hub::start();
    *hub.released.lock().expect("lock") = vec![entry(FIRST, None), tavily()];

    // Unconfigured, the service never asks.
    let plain = tempfile::tempdir().expect("tempdir");
    hub.enrol_managed(plain.path());
    write_service_config(plain.path(), 1);
    {
        let _service = Service::start(plain.path());
        let policy_requests = hub.policy_requests.load(Ordering::SeqCst);
        wait_until("an interval to pass", Duration::from_secs(15), || {
            hub.policy_requests.load(Ordering::SeqCst) > policy_requests
        });
        assert_eq!(hub.release_requests.load(Ordering::SeqCst), 0);
        assert!(!plain.path().join("supplier-credentials.json").exists());
    }

    let home = tempfile::tempdir().expect("tempdir");
    hub.enrol_managed(home.path());
    std::fs::write(
        home.path().join("hosted-service.json"),
        json!({
            "listen": "127.0.0.1:0", "origin": ORIGIN, "hosts": ["claude-connector"],
            "interval_seconds": 1, "supplier_custody": true,
        })
        .to_string(),
    )
    .expect("service configuration");
    let journal_path = home.path().join("journal.txt");
    let mut command = service_command(home.path());
    // The state under test is "no local key", so the test establishes it.
    command.env_remove("EXA_API_KEY");
    command.env_remove("TAVILY_API_KEY");
    let service = Service::start_with(
        command,
        Stdio::from(std::fs::File::create(&journal_path).expect("journal")),
    );
    let record = || -> Value {
        std::fs::read_to_string(home.path().join("supplier-credentials.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null)
    };
    // Fetched once at start, before the first session.
    assert_eq!(record()["held"][0]["connection_id"], "conn-exa-1");

    let authorization = hub.bearer("user-1", "claude-connector");
    let session = service.open_session("claude-connector", &authorization);
    let mut said = String::new();
    let mut call = |id: u64, name: &str, arguments: Value| -> Response {
        let response = service.call(
            "claude-connector",
            &authorization,
            &session,
            id,
            name,
            arguments,
        );
        said.push_str(&text(&response));
        response
    };

    let status = payload(&call(1, "context_status", json!({})));
    let exa = status["providers"]
        .as_array()
        .expect("providers")
        .iter()
        .find(|provider| provider["provider"] == "exa")
        .expect("exa is listed")
        .clone();
    assert_eq!(
        exa,
        json!({"provider": "exa", "configured": true, "source": "hub"})
    );
    assert_eq!(
        status["credentials"]["hub"][0]["connection_id"],
        "conn-exa-1"
    );

    // A refused crossing writes the session's opening records, the
    // provenance record among them, and crosses nothing.
    call(
        2,
        "context_fetch",
        json!({"url": "http://127.0.0.1:9/private"}),
    );
    let sessions = everything_under(&home.path().join("sessions"));
    assert!(sessions.contains("credentials_loaded"), "{sessions}");
    assert!(sessions.contains("conn-exa-1"), "{sessions}");

    // Rotation at the hub reaches the running service within an interval.
    *hub.released.lock().expect("lock") =
        vec![entry(ROTATED, Some("2026-09-19T12:00:00.000Z")), tavily()];
    wait_until(
        "the rotation to be fetched",
        Duration::from_secs(15),
        || record()["held"][0]["rotated_at"] == "2026-09-19T12:00:00.000Z",
    );
    let status = payload(&call(3, "context_status", json!({})));
    assert_eq!(
        status["credentials"]["hub"][0]["connection_id"],
        "conn-exa-1"
    );

    // Revocation: the hub serves nothing, and within an interval the same
    // session is refused by the provider's variable, with no word of a hub.
    hub.released.lock().expect("lock").clear();
    // Not the `accepted` outcome: at this interval the next fetch overwrites
    // it with `unchanged` a second later, and a stalled poll would miss it.
    wait_until(
        "the revocation to be fetched",
        Duration::from_secs(15),
        || {
            let record = record();
            record["http_status"] == 200 && record["held"] == json!([])
        },
    );
    let refused = call(
        4,
        "context_search",
        json!({"query": "anything", "provider": "exa"}),
    );
    let refusal = text(&refused);
    assert!(
        refusal.contains("unavailable: exa needs EXA_API_KEY"),
        "{refusal}"
    );
    assert!(!refusal.to_lowercase().contains("hub"), "{refusal}");

    // Let the relay run once more over the session's records, then stop.
    let asked = hub.policy_requests.load(Ordering::SeqCst);
    wait_until("another interval", Duration::from_secs(15), || {
        hub.policy_requests.load(Ordering::SeqCst) > asked + 1
    });
    drop(service);
    let (ok, stdout, _) = run(home.path(), &["status"]);
    assert!(ok, "{stdout}");
    assert!(stdout.contains("supplier credentials"), "{stdout}");
    assert!(stdout.contains("none held"), "{stdout}");
    said.push_str(&stdout);

    let journal = std::fs::read_to_string(&journal_path).expect("journal");
    assert!(
        journal.contains(
            "supplier credentials accepted 200; holding exa (conn-exa-1), tavily (conn-tavily-1)"
        ),
        "{journal}"
    );
    assert!(journal.contains("none held"), "{journal}");
    // The product's sweep for credential-shaped strings, over what the
    // contract names: the session records, the journal and the tool answers.
    std::fs::write(home.path().join("said.txt"), &said).expect("write");
    let session_files: Vec<String> = std::fs::read_dir(home.path().join("sessions"))
        .expect("sessions")
        .map(|entry| {
            format!(
                "sessions/{}",
                entry.expect("entry").file_name().to_string_lossy()
            )
        })
        .collect();
    let mut swept: Vec<&str> = session_files.iter().map(String::as_str).collect();
    swept.extend(["journal.txt", "said.txt", "supplier-credentials.json"]);
    credential_sweep::no_artefact_carries_a_credential(home.path(), &swept);

    let written = everything_under(home.path());
    for (what, text) in [
        ("a tool answer", &said),
        ("the operator home or the journal", &written),
    ] {
        for value in [FIRST, ROTATED, TAVILY] {
            assert!(!text.contains(value), "{what} carries a released value");
        }
    }
}

/// The maximum age of a kept release through the real binary and its real
/// clock, against this file's **test double** of the release route with a
/// `503` injected as the outage. At `interval_seconds: 1` the age is three
/// seconds. It establishes that a running service stops using a release the
/// hub has stopped confirming, says so, and takes it up again when the hub
/// answers. No supplier is called.
// Catches: a hub outage keeping a released key in use for ever; an expiry
// the fetch record, the journal or `context_status` does not state; a
// service that does not recover when the hub answers again.
#[test]
fn a_release_the_hub_stops_confirming_is_not_used_after_three_intervals() {
    const VALUE: &str = "tvly-released-9b1d44e2";
    let hub = Hub::start();
    *hub.released.lock().expect("lock") = vec![json!({
        "connection_id": "conn-tavily-1", "provider": "tavily",
        "variable": "TAVILY_API_KEY", "value": VALUE, "rotated_at": null,
    })];
    let home = tempfile::tempdir().expect("tempdir");
    hub.enrol_managed(home.path());
    std::fs::write(
        home.path().join("hosted-service.json"),
        json!({
            "listen": "127.0.0.1:0", "origin": ORIGIN, "hosts": ["claude-connector"],
            "interval_seconds": 1, "supplier_custody": true,
        })
        .to_string(),
    )
    .expect("service configuration");
    let journal_path = home.path().join("journal.txt");
    let mut command = service_command(home.path());
    command.env_remove("TAVILY_API_KEY");
    let service = Service::start_with(
        command,
        Stdio::from(std::fs::File::create(&journal_path).expect("journal")),
    );
    let record = || -> Value {
        std::fs::read_to_string(home.path().join("supplier-credentials.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null)
    };
    assert_eq!(record()["held"][0]["connection_id"], "conn-tavily-1");

    let authorization = hub.bearer("user-1", "claude-connector");
    let session = service.open_session("claude-connector", &authorization);
    let mut said = String::new();
    let mut call = |id: u64, name: &str, arguments: Value| -> Response {
        let response = service.call(
            "claude-connector",
            &authorization,
            &session,
            id,
            name,
            arguments,
        );
        said.push_str(&text(&response));
        response
    };

    hub.release_outage.store(true, Ordering::SeqCst);
    wait_until(
        "the release to pass its maximum age",
        Duration::from_secs(20),
        || record()["expired"][0]["connection_id"] == "conn-tavily-1",
    );
    let lapsed = record();
    assert_eq!(lapsed["outcome"], "unreachable", "{lapsed}");
    assert_eq!(lapsed["http_status"], 503);
    assert_eq!(lapsed["held"], json!([]));
    assert_eq!(lapsed["max_age_seconds"], 3);

    let status = payload(&call(1, "context_status", json!({})));
    let tavily = status["providers"]
        .as_array()
        .expect("providers")
        .iter()
        .find(|provider| provider["provider"] == "tavily")
        .expect("tavily is listed")
        .clone();
    assert_eq!(tavily["configured"], false, "{status}");
    assert_eq!(
        status["credentials"]["hub_expired"][0]["connection_id"],
        "conn-tavily-1"
    );
    let refusal = text(&call(
        2,
        "context_search",
        json!({"query": "anything", "provider": "tavily"}),
    ));
    assert!(
        refusal.contains("unavailable: tavily needs TAVILY_API_KEY"),
        "{refusal}"
    );
    assert!(!refusal.to_lowercase().contains("hub"), "{refusal}");

    // The hub answers again: the same value is taken up as a change.
    hub.release_outage.store(false, Ordering::SeqCst);
    wait_until(
        "the release to be held again",
        Duration::from_secs(15),
        || record()["held"][0]["connection_id"] == "conn-tavily-1",
    );
    let status = payload(&call(3, "context_status", json!({})));
    assert_eq!(
        status["credentials"]["hub"][0]["connection_id"],
        "conn-tavily-1"
    );

    drop(service);
    let journal = std::fs::read_to_string(&journal_path).expect("journal");
    assert!(
        journal.contains(
            "none held; no longer using tavily (conn-tavily-1), not confirmed by the hub for 3s"
        ),
        "{journal}"
    );
    let written = everything_under(home.path());
    for (what, text) in [
        ("a tool answer", &said),
        ("the operator home or the journal", &written),
    ] {
        assert!(!text.contains(VALUE), "{what} carries the released value");
    }
}
