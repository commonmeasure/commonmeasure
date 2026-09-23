//! The hosted edge driven as a host's cloud drives it: Streamable HTTP
//! against the real binary, a loopback origin standing for the open web, and
//! a loopback issuer standing for the hub's authorisation server, serving a
//! JWKS and signing access tokens with a real Ed25519 key.
//!
//! Nothing of the product is substituted: the listener, the token verifier,
//! the session table, the `McpServer`, the policy, the HTTP client and the
//! session log are the ones a deployed hosted edge runs. The origin and the
//! issuer are loopback servers rather than the open web and the hub, and the
//! policy opts into private addresses to reach the origin, as the stdio test
//! in `mediated_e2e.rs` does.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use commonmeasure_harness::identity::EdgeKey;
use commonmeasure_http::{Request, Response, Server, ServerHandle, send};
use serde_json::{Value, json};

/// The origin hosts reach the edge on. The process listens on loopback; the
/// origin is what tokens name and what the metadata publishes, as behind a
/// load balancer.
const ORIGIN: &str = "https://edge.test";
const ORGANISATION: &str = "org-42";
const KEY_ID: &str = "hub-key-1";

/// A loopback origin serving one body at `/` and counting the reads of that
/// page. The edge also reads the host's declarations (`robots.txt` and what
/// it names) before a fetch; those are not the crossing and are not counted.
struct Origin {
    handle: ServerHandle,
    hits: Arc<AtomicUsize>,
}

fn origin(body: &'static str) -> Origin {
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&hits);
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |request| {
            if request.target != "/" {
                return Response::text(404, "not here");
            }
            counted.fetch_add(1, Ordering::SeqCst);
            Response::text(200, body)
        })
        .expect("spawn");
    Origin { handle, hits }
}

/// The hub's authorisation server, as the edge sees it: RFC 8414 metadata
/// naming a JWKS, the JWKS carrying one Ed25519 key, and the key that signs
/// access tokens. Counts JWKS reads, because a crossing's ruling must not
/// cost one.
struct Issuer {
    handle: ServerHandle,
    key: EdgeKey,
    jwks_reads: Arc<AtomicUsize>,
}

impl Issuer {
    fn start() -> Self {
        let key = EdgeKey::generate().expect("a key");
        let jwk_x = key.jwk_x();
        let jwks_reads = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&jwks_reads);
        let listener = Server::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        let handle = listener
            .spawn(move |request| match request.target.as_str() {
                "/.well-known/oauth-authorization-server" => Response::json(
                    200,
                    &json!({
                        "issuer": url,
                        "jwks_uri": format!("{url}/oauth/jwks"),
                        "authorization_endpoint": format!("{url}/oauth/authorize"),
                        "token_endpoint": format!("{url}/oauth/token"),
                        "code_challenge_methods_supported": ["S256"],
                    })
                    .to_string(),
                ),
                "/oauth/jwks" => {
                    counted.fetch_add(1, Ordering::SeqCst);
                    Response::json(
                        200,
                        &json!({"keys": [
                            {"kty": "OKP", "crv": "Ed25519", "kid": KEY_ID, "x": jwk_x, "use": "sig", "alg": "EdDSA"}
                        ]})
                        .to_string(),
                    )
                }
                _ => Response::json(404, r#"{"detail":"not found"}"#),
            })
            .expect("spawn");
        Self {
            handle,
            key,
            jwks_reads,
        }
    }

    fn url(&self) -> String {
        self.handle.url()
    }

    /// The claims a token the hub issues for `subject` at `endpoint` carries.
    fn claims(&self, subject: &str, endpoint: &str) -> Value {
        let now = chrono::Utc::now().timestamp();
        json!({
            "iss": self.url(),
            "sub": subject,
            "aud": format!("{ORIGIN}/mcp/{endpoint}"),
            "org": ORGANISATION,
            "exp": now + 3600,
            "iat": now,
            "client_id": "https://claude.ai/.well-known/claude-client-metadata.json",
            "scope": "mcp",
        })
    }

    /// A token with these claims, signed by the issuer's key under its key id.
    fn token(&self, claims: &Value) -> String {
        self.token_with(
            &json!({"alg": "EdDSA", "typ": "JWT", "kid": KEY_ID}),
            claims,
            &self.key,
        )
    }

    fn token_with(&self, header: &Value, claims: &Value, key: &EdgeKey) -> String {
        let signed = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        let signature = key.sign(signed.as_bytes());
        format!("{signed}.{signature}")
    }

    fn bearer(&self, subject: &str, endpoint: &str) -> String {
        format!("Bearer {}", self.token(&self.claims(subject, endpoint)))
    }
}

/// An operator home enrolled with the loopback issuer as its hub, holding a
/// real edge key so the edge signs its fetches as an enrolled one does.
fn enrol(home: &Path, issuer: &Issuer) {
    let key = EdgeKey::generate().expect("a key");
    key.store(home).expect("stored");
    let hub = issuer.url();
    std::fs::write(
        home.join("enrolment.json"),
        json!({
            "hub": hub,
            "organization": {"id": ORGANISATION, "name": "Organisation 42"},
            "name": "hosted-test",
            "key_id": key.thumbprint(),
            "identity": {"origin": hub, "bot_page": format!("{hub}/bot")},
            "enrolled_at": "2026-09-15T00:00:00Z"
        })
        .to_string(),
    )
    .expect("enrolment written");
}

fn write_policy(home: &Path, policy: &str) {
    std::fs::write(home.join("policy.json"), policy).expect("policy written");
}

/// The real binary in its hosted mode on an OS-chosen loopback port.
struct Edge {
    child: Child,
    base: String,
}

impl Edge {
    fn start(home: &Path) -> Self {
        Self::start_with(home, &[])
    }

    fn start_with(home: &Path, extra: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
            .args([
                "hosted",
                "serve",
                "--listen",
                "127.0.0.1:0",
                "--origin",
                ORIGIN,
            ])
            .args(extra)
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
                .expect("hosted serve announces its address before serving")
                .expect("readable stdout");
            if let Some(address) = line.strip_prefix("listening on ") {
                break address.trim().to_owned();
            }
        };
        Self { child, base }
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Response {
        let mut request = match body {
            Some(body) => Request::post(path, body.to_string().into_bytes(), "application/json"),
            None => Request::get(path),
        };
        request.method = method.to_owned();
        request
            .headers
            .set("Accept", "application/json, text/event-stream");
        for (name, value) in headers {
            request.headers.set(name, value);
        }
        send(&format!("{}{path}", self.base), request).expect("the request completes")
    }

    fn post(&self, path: &str, headers: &[(&str, &str)], body: &Value) -> Response {
        self.request("POST", path, headers, Some(body))
    }

    /// Open a session on `endpoint` as the bearer, returning its id.
    fn open_session(&self, endpoint: &str, authorization: &str, client: Option<Value>) -> String {
        let response = self.post(
            &format!("/mcp/{endpoint}"),
            &[("Authorization", authorization)],
            &initialize(client),
        );
        assert_eq!(response.status, 200, "{}", text(&response));
        let body = body(&response);
        assert_eq!(body["result"]["serverInfo"]["name"], "commonmeasure");
        assert_eq!(body["result"]["protocolVersion"], "2025-06-18");
        let session = response
            .headers
            .get("Mcp-Session-Id")
            .expect("a session id is minted at initialize")
            .to_owned();
        assert!(session.starts_with("hosted-"), "{session}");
        let notified = self.post(
            &format!("/mcp/{endpoint}"),
            &[
                ("Authorization", authorization),
                ("Mcp-Session-Id", &session),
                ("MCP-Protocol-Version", "2025-06-18"),
            ],
            &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        );
        assert_eq!(notified.status, 202, "a notification is owed nothing");
        assert!(notified.body.is_empty());
        session
    }

    /// One request inside an open session.
    fn in_session(
        &self,
        endpoint: &str,
        authorization: &str,
        session: &str,
        message: &Value,
    ) -> Response {
        self.post(
            &format!("/mcp/{endpoint}"),
            &[
                ("Authorization", authorization),
                ("Mcp-Session-Id", session),
                ("MCP-Protocol-Version", "2025-06-18"),
            ],
            message,
        )
    }
}

impl Drop for Edge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn initialize(client: Option<Value>) -> Value {
    let mut params = json!({"protocolVersion": "2025-06-18", "capabilities": {}});
    if let Some(client) = client {
        params["clientInfo"] = client;
    }
    json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": params})
}

fn call(id: u64, name: &str, arguments: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
           "params": {"name": name, "arguments": arguments}})
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

fn session_files(home: &Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(home.join("sessions")) else {
        return Vec::new();
    };
    let mut files: Vec<_> = entries.map(|entry| entry.expect("entry").path()).collect();
    files.sort();
    files
}

fn records(home: &Path, session: &str) -> Vec<Value> {
    commonmeasure_harness::SessionLog::read(
        &home.join("sessions").join(format!("{session}.ndjson")),
    )
    .expect("the session file reads")
}

fn crossings(records: &[Value]) -> Vec<&Value> {
    records
        .iter()
        .filter(|record| {
            record["event"]
                .as_str()
                .is_some_and(|event| event.starts_with("crossing_"))
        })
        .collect()
}

#[test]
fn the_resource_metadata_names_each_endpoint_and_the_issuer_pinned_at_enrolment() {
    let issuer = Issuer::start();
    let home = tempfile::tempdir().expect("tempdir");
    enrol(home.path(), &issuer);
    let edge = Edge::start(home.path());

    for host in [
        "claude-connector",
        "chatgpt",
        "m365-copilot",
        "copilot-cloud-agent",
    ] {
        let response = edge.request(
            "GET",
            &format!("/.well-known/oauth-protected-resource/mcp/{host}"),
            &[],
            None,
        );
        assert_eq!(response.status, 200, "{}", text(&response));
        let metadata = body(&response);
        assert_eq!(metadata["resource"], format!("{ORIGIN}/mcp/{host}"));
        assert_eq!(metadata["authorization_servers"], json!([issuer.url()]));
        assert_eq!(metadata["bearer_methods_supported"], json!(["header"]));
    }
    let unknown = edge.request(
        "GET",
        "/.well-known/oauth-protected-resource/mcp/claude-code",
        &[],
        None,
    );
    assert_eq!(unknown.status, 404);
    assert_eq!(
        issuer.jwks_reads.load(Ordering::SeqCst),
        0,
        "metadata is served from the enrolment, not from the hub"
    );
}

#[test]
fn a_request_with_no_token_is_challenged_with_the_resource_metadata_and_nothing_is_served() {
    let issuer = Issuer::start();
    let home = tempfile::tempdir().expect("tempdir");
    enrol(home.path(), &issuer);
    let edge = Edge::start(home.path());

    let response = edge.post("/mcp/claude-connector", &[], &initialize(None));
    assert_eq!(response.status, 401, "{}", text(&response));
    assert_eq!(
        response.headers.get("WWW-Authenticate"),
        Some(&*format!(
            "Bearer resource_metadata=\"{ORIGIN}/.well-known/oauth-protected-resource/mcp/claude-connector\""
        ))
    );
    assert!(
        response.headers.get("Mcp-Session-Id").is_none(),
        "no session is opened for an unauthenticated request"
    );
    let basic = edge.post(
        "/mcp/claude-connector",
        &[("Authorization", "Basic dXNlcjpwYXNz")],
        &initialize(None),
    );
    assert_eq!(basic.status, 401);
    assert!(
        body(&basic)["error_description"]
            .as_str()
            .is_some_and(|reason| reason.contains("Bearer")),
        "{}",
        text(&basic)
    );
    assert!(session_files(home.path()).is_empty());
}

/// The assertions of `mediated_e2e.rs` carried over the hosted transport:
/// the tools list with `readOnlyHint`, the fetch returns the bytes and the
/// hash of exactly what was delivered, the crossing is recorded under the
/// token's subject with its basis, the client is identified once, and the
/// session id in the header is the one on every record.
#[test]
fn a_mediated_fetch_over_http_returns_the_bytes_and_records_the_crossing_under_the_subject() {
    let issuer = Issuer::start();
    let origin = origin("Ofgem sets the cap quarterly.");
    let home = tempfile::tempdir().expect("tempdir");
    enrol(home.path(), &issuer);
    write_policy(
        home.path(),
        r#"{"policy_mode":"strict","allow_private_hosts":true}"#,
    );
    let edge = Edge::start(home.path());
    let authorization = issuer.bearer("user-1", "claude-connector");
    let client = json!({"name": "claude-ai", "version": "1.0"});
    let session = edge.open_session("claude-connector", &authorization, Some(client.clone()));

    let listed = edge.in_session(
        "claude-connector",
        &authorization,
        &session,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
    );
    assert_eq!(listed.status, 200);
    let tools = body(&listed)["result"]["tools"].clone();
    let hints: Vec<(String, Value)> = tools
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| {
            (
                tool["name"].as_str().expect("name").to_owned(),
                tool["annotations"]["readOnlyHint"].clone(),
            )
        })
        .collect();
    assert_eq!(
        hints,
        [
            ("context_fetch".to_owned(), json!(true)),
            ("context_search".to_owned(), json!(true)),
            ("context_status".to_owned(), json!(true)),
        ],
        "the three tools that read are served, each declaring readOnlyHint; context_enrol is \
         not advertised on a hosted endpoint"
    );
    let enrol = edge.in_session(
        "claude-connector",
        &authorization,
        &session,
        &call(9, "context_enrol", json!({"directory": "/work"})),
    );
    assert_eq!(body(&enrol)["result"]["isError"], true);
    assert!(
        text(&enrol).contains("unknown tool context_enrol"),
        "{}",
        text(&enrol)
    );

    let fetched = edge.in_session(
        "claude-connector",
        &authorization,
        &session,
        &call(2, "context_fetch", json!({"url": origin.handle.url()})),
    );
    assert_eq!(fetched.status, 200, "{}", text(&fetched));
    assert_eq!(body(&fetched)["result"]["isError"], false);
    let result = payload(&fetched);
    assert_eq!(result["content"], "Ofgem sets the cap quarterly.");
    assert_eq!(result["http_status"], 200);
    let content_hash = result["content_hash"].as_str().expect("hash").to_owned();
    assert!(content_hash.starts_with("sha256:"));
    assert_eq!(
        content_hash,
        commonmeasure_types::canonical::sha256_digest(b"Ofgem sets the cap quarterly."),
        "the agent is told the hash of exactly what it received"
    );
    let again = edge.in_session(
        "claude-connector",
        &authorization,
        &session,
        &call(3, "context_fetch", json!({"url": origin.handle.url()})),
    );
    assert_eq!(body(&again)["result"]["isError"], false);
    assert_eq!(origin.hits.load(Ordering::SeqCst), 2);

    let files = session_files(home.path());
    assert_eq!(
        files,
        [home
            .path()
            .join("sessions")
            .join(format!("{session}.ndjson"))],
        "one session, one file, named by the id the header carried"
    );
    let recorded = records(home.path(), &session);
    assert!(
        recorded.iter().all(|record| {
            record["payload"]["session_id"]
                .as_str()
                .is_none_or(|named| named == session)
        }),
        "every record that names a session names this one"
    );
    let identity = recorded
        .iter()
        .find(|record| record["event"] == "edge_identity")
        .expect("the enrolled identity is recorded");
    assert_eq!(identity["payload"]["host"], "claude-connector");
    assert_eq!(identity["payload"]["session_id"], session);
    let identified: Vec<&Value> = recorded
        .iter()
        .filter(|record| record["event"] == "client_identified")
        .collect();
    assert_eq!(identified.len(), 1, "written once, not per call");
    assert_eq!(identified[0]["payload"]["client"], client);
    assert_eq!(identified[0]["payload"]["protocol_version"], "2025-06-18");
    let crossings = crossings(&recorded);
    assert_eq!(crossings.len(), 2);
    for crossing in &crossings {
        assert_eq!(crossing["event"], "crossing_mediated");
        assert_eq!(crossing["payload"]["session_id"], session);
        assert_eq!(crossing["payload"]["host"], "claude-connector");
        assert_eq!(crossing["payload"]["content_hash"], content_hash);
        assert_eq!(crossing["payload"]["principal"], "subject:user-1");
        assert_eq!(crossing["payload"]["authentication_basis"], "oauth_subject");
        assert_eq!(crossing["payload"]["client"], client);
        assert!(
            crossing["payload"].get("cwd").is_none_or(Value::is_null),
            "a hosted session has no working directory: {}",
            crossing["payload"]
        );
    }
    assert_eq!(
        issuer.jwks_reads.load(Ordering::SeqCst),
        1,
        "the issuer's keys were read once, at the first token, and never in a crossing"
    );
    assert!(home.path().join("hosted-jwks.json").exists());

    let status = edge.in_session(
        "claude-connector",
        &authorization,
        &session,
        &call(4, "context_status", json!({})),
    );
    let status = payload(&status);
    assert_eq!(status["session_id"], session);
    assert_eq!(status["client"], client);
}

/// A policy binding renames the subject and the edge-token label, and each
/// crossing records the label with the basis that authenticated it.
#[test]
fn a_bound_subject_and_a_bound_edge_token_are_recorded_by_their_labels_and_bases() {
    let issuer = Issuer::start();
    let origin = origin("bound");
    let home = tempfile::tempdir().expect("tempdir");
    enrol(home.path(), &issuer);
    write_policy(
        home.path(),
        r#"{"policy_mode":"strict","allow_private_hosts":true,
            "principals":[
              {"principal":"research-lead","subject":"user-1"},
              {"principal":"ci-docs","edge_token":"ci:acme/docs"}
            ]}"#,
    );
    let issued = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["hosted", "token", "issue", "ci:acme/docs"])
        .env("COMMONMEASURE_HOME", home.path())
        .output()
        .expect("issue runs");
    assert!(
        issued.status.success(),
        "{}",
        String::from_utf8_lossy(&issued.stderr)
    );
    let edge_token = String::from_utf8(issued.stdout)
        .expect("utf8")
        .trim()
        .to_owned();
    assert!(edge_token.starts_with("cmet_"), "{edge_token}");
    assert!(
        !std::fs::read_to_string(home.path().join("hosted-tokens.json"))
            .expect("token file")
            .contains(&edge_token),
        "only the hash is stored"
    );
    let edge = Edge::start(home.path());

    let subject = issuer.bearer("user-1", "chatgpt");
    let subject_session = edge.open_session("chatgpt", &subject, None);
    let fetched = edge.in_session(
        "chatgpt",
        &subject,
        &subject_session,
        &call(1, "context_fetch", json!({"url": origin.handle.url()})),
    );
    assert_eq!(
        body(&fetched)["result"]["isError"],
        false,
        "{}",
        text(&fetched)
    );

    let bearer = format!("Bearer {edge_token}");
    let token_session = edge.open_session("copilot-cloud-agent", &bearer, None);
    let fetched = edge.in_session(
        "copilot-cloud-agent",
        &bearer,
        &token_session,
        &call(1, "context_fetch", json!({"url": origin.handle.url()})),
    );
    assert_eq!(
        body(&fetched)["result"]["isError"],
        false,
        "{}",
        text(&fetched)
    );

    let by_subject = crossings(&records(home.path(), &subject_session))[0].clone();
    assert_eq!(by_subject["payload"]["host"], "chatgpt");
    assert_eq!(by_subject["payload"]["principal"], "research-lead");
    assert_eq!(
        by_subject["payload"]["authentication_basis"],
        "oauth_subject"
    );
    let by_token = crossings(&records(home.path(), &token_session))[0].clone();
    assert_eq!(by_token["payload"]["host"], "copilot-cloud-agent");
    assert_eq!(by_token["payload"]["principal"], "ci-docs");
    assert_eq!(by_token["payload"]["authentication_basis"], "edge_token");

    // A subject the policy does not bind fails closed: the policy declares
    // principals, so an unbound one is refused before any crossing.
    let stranger = issuer.bearer("user-9", "chatgpt");
    let stranger_session = edge.open_session("chatgpt", &stranger, None);
    let refused = edge.in_session(
        "chatgpt",
        &stranger,
        &stranger_session,
        &call(1, "context_fetch", json!({"url": origin.handle.url()})),
    );
    assert_eq!(
        body(&refused)["result"]["isError"],
        true,
        "{}",
        text(&refused)
    );
    let detail = payload(&refused)["error"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(detail.contains("user-9"), "{detail}");
    assert_eq!(origin.hits.load(Ordering::SeqCst), 2);
}

#[test]
fn strict_policy_refuses_the_crossing_before_the_origin_is_reached_and_records_the_refusal() {
    let issuer = Issuer::start();
    let origin = origin("secret");
    let home = tempfile::tempdir().expect("tempdir");
    enrol(home.path(), &issuer);
    write_policy(
        home.path(),
        r#"{"policy_mode":"strict","allow_private_hosts":true,
            "constraints":[{"kind":"allowed_source_host","host":"www.gov.uk"}]}"#,
    );
    let edge = Edge::start(home.path());
    let authorization = issuer.bearer("user-1", "m365-copilot");
    let session = edge.open_session("m365-copilot", &authorization, None);

    let refused = edge.in_session(
        "m365-copilot",
        &authorization,
        &session,
        &call(1, "context_fetch", json!({"url": origin.handle.url()})),
    );
    assert_eq!(
        refused.status, 200,
        "a refusal is a tool result the agent reads"
    );
    assert_eq!(body(&refused)["result"]["isError"], true);
    let detail = payload(&refused)["error"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(detail.contains("refused before the crossing"), "{detail}");
    assert_eq!(origin.hits.load(Ordering::SeqCst), 0, "no bytes moved");

    let recorded = records(home.path(), &session);
    let crossings = crossings(&recorded);
    assert_eq!(crossings.len(), 1);
    assert_eq!(crossings[0]["event"], "crossing_refused");
    assert_eq!(crossings[0]["payload"]["host"], "m365-copilot");
    assert_eq!(crossings[0]["payload"]["principal"], "subject:user-1");
    assert!(crossings[0]["payload"]["refusal"].is_string());
    assert!(crossings[0]["payload"]["content_hash"].is_null());
}

/// The hosted edge rules `robots.txt` as the stdio edge does, in every mode:
/// a `Disallow` refuses before the page is requested (WP-29), and so do a
/// file that answers 503 and a file whose redirect this edge declines, with
/// no earlier answer held (RFC 9309 §2.3.1.2, §2.3.1.4).
#[test]
fn a_disallow_or_an_unreachable_robots_file_refuses_the_hosted_crossing_in_observe() {
    robots_refusals_on_the_hosted_edge("observe");
}

#[test]
fn a_disallow_or_an_unreachable_robots_file_refuses_the_hosted_crossing_in_prefer() {
    robots_refusals_on_the_hosted_edge("prefer");
}

#[test]
fn a_disallow_or_an_unreachable_robots_file_refuses_the_hosted_crossing_in_strict() {
    robots_refusals_on_the_hosted_edge("strict");
}

/// One origin per case, bound before the edge starts so the policy can name
/// each origin's prefix as mediated. `localhost` is outside those prefixes,
/// so the third origin's `robots.txt` redirect names an address this edge
/// does not mediate, and is declined in every mode.
fn robots_refusals_on_the_hosted_edge(mode: &str) {
    let issuer = Issuer::start();
    let home = tempfile::tempdir().expect("tempdir");
    enrol(home.path(), &issuer);

    let unmediated_hits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&unmediated_hits);
    let unmediated = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            Response::text(200, "User-agent: *\nAllow: /\n")
        })
        .expect("spawn");
    let declined_to = format!("http://localhost:{}/robots.txt", unmediated.addr().port());
    let mut redirect = Response::new(301, Vec::new());
    redirect.headers.set("Location", &declined_to);

    let pages = Arc::new(AtomicUsize::new(0));
    let cases = [
        (
            Response::text(200, "User-agent: *\nDisallow: /\n"),
            "A `Disallow` binds in every policy mode: refused before the request.",
            "refused",
        ),
        (
            Response::text(503, "down"),
            "An unreachable robots.txt is a complete disallow in every policy mode",
            "unreachable",
        ),
        (redirect, "which this edge does not follow", "unreachable"),
    ]
    .map(|(robots, expected, outcome)| {
        let counted = Arc::clone(&pages);
        let site = Server::bind("127.0.0.1:0")
            .expect("bind")
            .spawn(move |request| {
                if request.target == "/robots.txt" {
                    return robots.clone();
                }
                counted.fetch_add(1, Ordering::SeqCst);
                Response::text(200, "the page")
            })
            .expect("spawn");
        (site, expected, outcome)
    });
    let prefixes: Vec<String> = cases
        .iter()
        .map(|(site, _, _)| format!("{}/", site.url()))
        .collect();
    write_policy(
        home.path(),
        &json!({"policy_mode": mode, "record_internal_prefixes": prefixes}).to_string(),
    );
    let edge = Edge::start(home.path());
    let authorization = issuer.bearer("user-3", "claude-connector");
    let session = edge.open_session("claude-connector", &authorization, None);

    for (id, (site, expected, outcome)) in (1..).zip(&cases) {
        let refused = edge.in_session(
            "claude-connector",
            &authorization,
            &session,
            &call(
                id,
                "context_fetch",
                json!({"url": format!("{}/article", site.url())}),
            ),
        );
        assert_eq!(
            body(&refused)["result"]["isError"],
            true,
            "{mode} {outcome}: {}",
            text(&refused)
        );
        let detail = text(&refused);
        assert!(detail.contains(expected), "{mode} {outcome}: {detail}");
        assert!(
            detail.contains(&format!("{}/robots.txt", site.url())),
            "{mode} {outcome}: {detail}"
        );
    }
    assert_eq!(pages.load(Ordering::SeqCst), 0, "{mode}: no page request");
    assert_eq!(
        unmediated_hits.load(Ordering::SeqCst),
        0,
        "{mode}: the declined redirect was not followed"
    );

    let recorded = records(home.path(), &session);
    let crossings = crossings(&recorded);
    assert_eq!(crossings.len(), 3, "{mode}");
    for (crossing, outcome) in crossings
        .iter()
        .zip(["refused", "unreachable", "unreachable"])
    {
        assert_eq!(crossing["event"], "crossing_refused", "{mode} {outcome}");
        let robots = &crossing["payload"]["declarations"]["robots"];
        assert_eq!(robots["mode"], mode, "{mode} {outcome}");
        assert_eq!(robots["outcome"], outcome, "{mode}: {robots}");
    }
}

/// On an enrolled edge, opening a session records the edge's identity, so
/// an idle session leaves a file holding that record alone: no client, no
/// credentials, no crossing. The stdio server on an unenrolled home leaves
/// no file at all (`mediated_e2e.rs`); the hosted edge is always enrolled.
#[test]
fn an_initialised_session_that_is_asked_for_nothing_leaves_no_file() {
    let issuer = Issuer::start();
    let home = tempfile::tempdir().expect("tempdir");
    enrol(home.path(), &issuer);
    let edge = Edge::start(home.path());
    let authorization = issuer.bearer("user-1", "claude-connector");
    let session = edge.open_session(
        "claude-connector",
        &authorization,
        Some(json!({"name": "claude-ai"})),
    );
    let listed = edge.in_session(
        "claude-connector",
        &authorization,
        &session,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
    );
    assert_eq!(listed.status, 200);
    assert_eq!(
        body(&listed)["result"]["tools"].as_array().map(Vec::len),
        Some(3)
    );
    let pinged = edge.in_session(
        "claude-connector",
        &authorization,
        &session,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}),
    );
    assert_eq!(body(&pinged)["result"], json!({}));
    assert!(
        session_files(home.path()).is_empty(),
        "initialize, notifications/initialized, tools/list and ping open nothing on disk on an \
         enrolled edge: {:?}",
        session_files(home.path())
    );

    // The first request that is not lifecycle opens the session: the file
    // appears, its first record is the identity recorded at open, and the
    // client the server holds is the one initialize named.
    let status = edge.in_session(
        "claude-connector",
        &authorization,
        &session,
        &call(3, "context_status", json!({})),
    );
    assert_eq!(status.status, 200, "{}", text(&status));
    let reported = payload(&status);
    assert_eq!(reported["session_id"], session);
    assert_eq!(reported["client"]["name"], "claude-ai");
    assert_eq!(
        reported["policy"]["private_floor"], "policy",
        "hosted serve leaves the floor to the policy; the service holds it"
    );
    let files = session_files(home.path());
    assert_eq!(files.len(), 1, "{files:?}");
    let events: Vec<String> = records(home.path(), &session)
        .iter()
        .map(|record| record["event"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(
        events,
        ["edge_identity"],
        "context_status leaves no record of its own; the identity recorded at open is the first"
    );
}

#[test]
fn two_concurrent_sessions_leave_two_files_each_with_its_own_crossing() {
    let issuer = Issuer::start();
    let origin = origin("shared origin");
    let home = tempfile::tempdir().expect("tempdir");
    enrol(home.path(), &issuer);
    write_policy(
        home.path(),
        r#"{"policy_mode":"strict","allow_private_hosts":true}"#,
    );
    let edge = Edge::start(home.path());

    let sessions: Vec<(String, String)> = std::thread::scope(|scope| {
        let workers: Vec<_> = ["user-1", "user-2"]
            .into_iter()
            .map(|subject| {
                let edge = &edge;
                let issuer = &issuer;
                let origin = origin.handle.url();
                scope.spawn(move || {
                    let authorization = issuer.bearer(subject, "claude-connector");
                    let session = edge.open_session("claude-connector", &authorization, None);
                    for id in 1..=3 {
                        let fetched = edge.in_session(
                            "claude-connector",
                            &authorization,
                            &session,
                            &call(id, "context_fetch", json!({"url": origin})),
                        );
                        assert_eq!(fetched.status, 200, "{}", text(&fetched));
                        assert_eq!(body(&fetched)["result"]["isError"], false);
                    }
                    (subject.to_owned(), session)
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a session completes"))
            .collect()
    });

    assert_ne!(sessions[0].1, sessions[1].1, "two sessions, two ids");
    let files = session_files(home.path());
    assert_eq!(files.len(), 2, "{files:?}");
    for (subject, session) in &sessions {
        let recorded = records(home.path(), session);
        let crossings = crossings(&recorded);
        assert_eq!(crossings.len(), 3, "{session}");
        for crossing in crossings {
            assert_eq!(crossing["event"], "crossing_mediated");
            assert_eq!(crossing["payload"]["session_id"], *session);
            assert_eq!(
                crossing["payload"]["principal"],
                format!("subject:{subject}")
            );
        }
    }
    assert_eq!(origin.hits.load(Ordering::SeqCst), 6);
}

#[test]
fn one_pace_per_host_is_shared_between_tenants_and_the_refusal_names_no_time() {
    let issuer = Issuer::start();
    // A source with a delay: one pace per host for the whole edge, because
    // one enrolment and one key id are what its log sees.
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(|request| {
            if request.target == "/robots.txt" {
                return Response::text(200, "User-agent: *\nAllow: /\nCrawl-delay: 60\n");
            }
            Response::text(200, "the page")
        })
        .expect("spawn");
    let home = tempfile::tempdir().expect("tempdir");
    enrol(home.path(), &issuer);
    write_policy(
        home.path(),
        r#"{"policy_mode":"observe","allow_private_hosts":true}"#,
    );
    // The turn another tenant's session left, half a minute ahead.
    let store = commonmeasure_harness::crawl_delay::CrawlDelayStore::open(home.path());
    let record = store.path_of("127.0.0.1");
    std::fs::create_dir_all(record.parent().expect("a directory")).expect("the directory");
    std::fs::write(
        &record,
        serde_json::to_vec(&json!({
            "host": "127.0.0.1",
            "at": chrono::Utc::now() + chrono::Duration::seconds(30),
        }))
        .expect("a record"),
    )
    .expect("the record");

    let edge = Edge::start(home.path());
    let authorization = issuer.bearer("user-2", "claude-connector");
    let session = edge.open_session("claude-connector", &authorization, None);
    let refused = edge.in_session(
        "claude-connector",
        &authorization,
        &session,
        &call(1, "context_fetch", json!({"url": handle.url()})),
    );
    let detail = text(&refused);
    assert!(detail.contains("Crawl-delay: 60"), "{detail}");
    assert!(detail.contains("127.0.0.1"), "{detail}");
    assert!(
        !detail.contains("may be sent"),
        "the refusal names another tenant's fetch time: {detail}"
    );
    assert!(
        detail.contains("served over HTTP"),
        "the refusal does not name the transport: {detail}"
    );
    // The delay's own text names no store path. (The suffix naming the
    // session log is common to every refusal and is not this text's.)
    assert!(
        !detail.contains("crawl-delay"),
        "the refusal names the operator's store: {detail}"
    );

    let recorded = records(home.path(), &session);
    let crossings = crossings(&recorded);
    let delay = &crossings[0]["payload"]["declarations"]["robots"]["delay"];
    assert_eq!(delay["outcome"], "refused", "{delay}");
    assert!(delay["next_at"].is_null(), "{delay}");
    assert!(delay["wait_ms"].is_null(), "{delay}");
    // A hosted call may spend 20 seconds asleep, well inside the 60 seconds
    // the HTTP transport allows a tool call.
    assert_eq!(delay["budget_ms"], 20_000, "{delay}");
}

#[test]
fn an_unknown_path_a_foreign_origin_a_get_a_bad_version_and_a_stale_session_are_refused() {
    let issuer = Issuer::start();
    let home = tempfile::tempdir().expect("tempdir");
    enrol(home.path(), &issuer);
    let edge = Edge::start_with(
        home.path(),
        &["--allow-origin", "https://console.edge.test"],
    );
    let authorization = issuer.bearer("user-1", "chatgpt");
    let auth = [("Authorization", authorization.as_str())];

    // An unknown path is 404, naming the endpoints: the stdio server's
    // unknown --host in HTTP form.
    let unknown = edge.post("/mcp/claude-code", &auth, &initialize(None));
    assert_eq!(unknown.status, 404, "{}", text(&unknown));
    assert!(text(&unknown).contains("/mcp/chatgpt"));
    assert_eq!(edge.post("/", &auth, &initialize(None)).status, 404);

    // An Origin that is not this edge's own nor an allowed one is 403,
    // before the token is even read.
    let foreign = edge.post(
        "/mcp/chatgpt",
        &[("Origin", "https://evil.test")],
        &initialize(None),
    );
    assert_eq!(foreign.status, 403, "{}", text(&foreign));
    assert!(text(&foreign).contains("evil.test"));
    let null_origin = edge.post("/mcp/chatgpt", &[("Origin", "null")], &initialize(None));
    assert_eq!(null_origin.status, 403);
    for allowed in [ORIGIN, "https://console.edge.test"] {
        let own = edge.post(
            "/mcp/chatgpt",
            &[("Authorization", &authorization), ("Origin", allowed)],
            &initialize(None),
        );
        assert_eq!(own.status, 200, "Origin {allowed}: {}", text(&own));
    }

    // GET offers no event stream here.
    let get = edge.request("GET", "/mcp/chatgpt", &auth, None);
    assert_eq!(get.status, 405);
    assert_eq!(get.headers.get("Allow"), Some("POST, DELETE"));
    assert_eq!(edge.request("PUT", "/mcp/chatgpt", &auth, None).status, 405);

    // A protocol version this edge does not serve is 400, by name.
    let session = edge.open_session("chatgpt", &authorization, None);
    let old = edge.post(
        "/mcp/chatgpt",
        &[
            ("Authorization", &authorization),
            ("Mcp-Session-Id", &session),
            ("MCP-Protocol-Version", "2024-11-05"),
        ],
        &json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}),
    );
    assert_eq!(old.status, 400, "{}", text(&old));
    assert!(text(&old).contains("2024-11-05"));
    let latest = edge.post(
        "/mcp/chatgpt",
        &[
            ("Authorization", &authorization),
            ("Mcp-Session-Id", &session),
            ("MCP-Protocol-Version", "2025-11-25"),
        ],
        &json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}),
    );
    assert_eq!(latest.status, 200, "{}", text(&latest));

    // The hosted set serves 2025-06-18 and 2025-11-25 only: a client asking
    // for 2025-03-26 in the header is refused, and one asking for it at
    // initialize is answered with the latest served, as the protocol directs
    // for a revision the server does not serve.
    let batching = edge.post(
        "/mcp/chatgpt",
        &[
            ("Authorization", &authorization),
            ("Mcp-Session-Id", &session),
            ("MCP-Protocol-Version", "2025-03-26"),
        ],
        &json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}),
    );
    assert_eq!(batching.status, 400, "{}", text(&batching));
    assert!(text(&batching).contains("2025-03-26"));
    let mut old_initialize = initialize(None);
    old_initialize["params"]["protocolVersion"] = json!("2025-03-26");
    let negotiated = edge.post("/mcp/chatgpt", &auth, &old_initialize);
    assert_eq!(negotiated.status, 200, "{}", text(&negotiated));
    assert_eq!(body(&negotiated)["result"]["protocolVersion"], "2025-11-25");
    assert!(
        body(&negotiated)["result"]["serverInfo"]["title"].is_string(),
        "the answer is in the negotiated revision's shape"
    );

    // A request outside a session must be initialize.
    let no_session = edge.post(
        "/mcp/chatgpt",
        &auth,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}),
    );
    assert_eq!(no_session.status, 400, "{}", text(&no_session));
    assert!(text(&no_session).contains("Mcp-Session-Id"));
    let garbage = edge.request(
        "POST",
        "/mcp/chatgpt",
        &auth,
        Some(&Value::String("not a message".to_owned())),
    );
    assert_eq!(garbage.status, 400, "{}", text(&garbage));

    // A session nobody opened is 404; a session on another endpoint is 404.
    let unknown = edge.post(
        "/mcp/chatgpt",
        &[
            ("Authorization", &authorization),
            (
                "Mcp-Session-Id",
                "hosted-0-00000000000000000000000000000000",
            ),
        ],
        &json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}),
    );
    assert_eq!(unknown.status, 404, "{}", text(&unknown));
    let elsewhere = issuer.bearer("user-1", "claude-connector");
    let other_endpoint = edge.post(
        "/mcp/claude-connector",
        &[("Authorization", &elsewhere), ("Mcp-Session-Id", &session)],
        &json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}),
    );
    assert_eq!(other_endpoint.status, 404, "{}", text(&other_endpoint));

    // DELETE ends the session; its id then answers 404.
    let ended = edge.request(
        "DELETE",
        "/mcp/chatgpt",
        &[
            ("Authorization", &authorization),
            ("Mcp-Session-Id", &session),
        ],
        None,
    );
    assert_eq!(ended.status, 200, "{}", text(&ended));
    let after = edge.in_session(
        "chatgpt",
        &authorization,
        &session,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}),
    );
    assert_eq!(after.status, 404, "{}", text(&after));
    let twice = edge.request(
        "DELETE",
        "/mcp/chatgpt",
        &[
            ("Authorization", &authorization),
            ("Mcp-Session-Id", &session),
        ],
        None,
    );
    assert_eq!(twice.status, 404);
    for file in session_files(home.path()) {
        let recorded = commonmeasure_harness::SessionLog::read(&file).expect("reads");
        assert!(
            crossings(&recorded).is_empty(),
            "nothing was fetched: {}",
            file.display()
        );
    }
}

/// Every token check refused by name, each a 401 whose challenge and body
/// say which check failed; and a session id presented under another
/// subject's token is unknown.
#[test]
fn each_token_check_is_refused_by_name() {
    let issuer = Issuer::start();
    let home = tempfile::tempdir().expect("tempdir");
    enrol(home.path(), &issuer);
    let edge = Edge::start(home.path());
    let endpoint = "/mcp/claude-connector";
    let good = issuer.claims("user-1", "claude-connector");
    let now = chrono::Utc::now().timestamp();
    let other_key = EdgeKey::generate().expect("a key");
    let header = json!({"alg": "EdDSA", "typ": "JWT", "kid": KEY_ID});

    let mut wrong_audience = good.clone();
    wrong_audience["aud"] = json!(format!("{ORIGIN}/mcp/chatgpt"));
    let mut wrong_issuer = good.clone();
    wrong_issuer["iss"] = json!("https://other-hub.test");
    let mut wrong_organisation = good.clone();
    wrong_organisation["org"] = json!("org-43");
    let mut no_organisation = good.clone();
    no_organisation
        .as_object_mut()
        .expect("object")
        .remove("org");
    let mut expired = good.clone();
    expired["exp"] = json!(now - 120);
    let mut no_expiry = good.clone();
    no_expiry.as_object_mut().expect("object").remove("exp");
    let mut not_yet = good.clone();
    not_yet["nbf"] = json!(now + 600);
    let mut empty_subject = good.clone();
    empty_subject["sub"] = json!("");
    let mut no_subject = good.clone();
    no_subject.as_object_mut().expect("object").remove("sub");

    let cases: Vec<(&str, String, &str)> = vec![
        ("wrong audience", issuer.token(&wrong_audience), "audience"),
        ("wrong issuer", issuer.token(&wrong_issuer), "issuer"),
        (
            "wrong organisation",
            issuer.token(&wrong_organisation),
            "is not the enrolled organisation org-42",
        ),
        (
            "no organisation",
            issuer.token(&no_organisation),
            "names no organisation",
        ),
        ("expired", issuer.token(&expired), "expired"),
        ("no expiry", issuer.token(&no_expiry), "no expiry"),
        ("not yet valid", issuer.token(&not_yet), "not valid before"),
        (
            "empty subject",
            issuer.token(&empty_subject),
            "empty or missing subject",
        ),
        (
            "no subject",
            issuer.token(&no_subject),
            "empty or missing subject",
        ),
        (
            "another key under the published key id",
            issuer.token_with(&header, &good, &other_key),
            "signature does not verify",
        ),
        (
            "a key id the issuer does not publish",
            issuer.token_with(
                &json!({"alg": "EdDSA", "kid": "hub-key-9"}),
                &good,
                &issuer.key,
            ),
            "signing key hub-key-9",
        ),
        (
            "an algorithm that is not EdDSA",
            issuer.token_with(&json!({"alg": "RS256", "kid": KEY_ID}), &good, &issuer.key),
            "RS256",
        ),
        (
            "no key id",
            issuer.token_with(&json!({"alg": "EdDSA"}), &good, &issuer.key),
            "no signing key",
        ),
        (
            "not a JWT",
            "opaque-token".to_owned(),
            "three base64url segments",
        ),
        (
            "an edge token this edge never issued",
            "cmet_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_owned(),
            "not one this edge issued",
        ),
    ];
    for (case, token, named) in &cases {
        let response = edge.post(
            endpoint,
            &[("Authorization", &format!("Bearer {token}"))],
            &initialize(None),
        );
        assert_eq!(response.status, 401, "{case}: {}", text(&response));
        let challenge = response
            .headers
            .get("WWW-Authenticate")
            .unwrap_or_else(|| panic!("{case}: a challenge"));
        assert!(
            challenge.starts_with(&format!(
                "Bearer resource_metadata=\"{ORIGIN}/.well-known/oauth-protected-resource/mcp/claude-connector\", error=\"invalid_token\", error_description=\""
            )),
            "{case}: {challenge}"
        );
        assert!(challenge.contains(named), "{case}: {challenge}");
        let body = body(&response);
        assert_eq!(body["error"], "invalid_token", "{case}");
        assert!(
            body["error_description"]
                .as_str()
                .is_some_and(|reason| reason.contains(named)),
            "{case}: {body}"
        );
        assert!(
            response.headers.get("Mcp-Session-Id").is_none(),
            "{case}: no session is opened"
        );
    }
    assert!(
        session_files(home.path()).is_empty(),
        "a refused token opens no session, so nothing is recorded"
    );

    // The good token, and one whose audience lists this endpoint among
    // others, open a session.
    let session = edge.open_session(
        "claude-connector",
        &format!("Bearer {}", issuer.token(&good)),
        None,
    );
    let mut many_audiences = good.clone();
    many_audiences["aud"] = json!([
        format!("{ORIGIN}/mcp/chatgpt"),
        format!("{ORIGIN}/mcp/claude-connector")
    ]);
    edge.open_session(
        "claude-connector",
        &format!("Bearer {}", issuer.token(&many_audiences)),
        None,
    );

    // The session id is not authentication: under another subject's valid
    // token it is unknown.
    let other = issuer.bearer("user-2", "claude-connector");
    let hijack = edge.in_session(
        "claude-connector",
        &other,
        &session,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}),
    );
    assert_eq!(hijack.status, 404, "{}", text(&hijack));
    let owner = format!("Bearer {}", issuer.token(&good));
    let still = edge.in_session(
        "claude-connector",
        &owner,
        &session,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}),
    );
    assert_eq!(
        still.status,
        200,
        "the owner's session continues: {}",
        text(&still)
    );
    assert_eq!(
        issuer.jwks_reads.load(Ordering::SeqCst),
        1,
        "one JWKS read for the first published key id; the unpublished key id waits out the refetch interval"
    );
}

#[test]
fn a_revoked_edge_token_is_refused_by_name_at_its_next_request() {
    let issuer = Issuer::start();
    let origin = origin("cloud agent");
    let home = tempfile::tempdir().expect("tempdir");
    enrol(home.path(), &issuer);
    write_policy(
        home.path(),
        r#"{"policy_mode":"strict","allow_private_hosts":true}"#,
    );
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
            .args(["hosted", "token"])
            .args(args)
            .env("COMMONMEASURE_HOME", home.path())
            .output()
            .expect("runs");
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        )
    };
    let (ok, token, _) = run(&["issue", "ci:acme/site"]);
    assert!(ok);
    let (ok, _, again) = run(&["issue", "ci:acme/site"]);
    assert!(!ok, "a label is issued once");
    assert!(again.contains("already issued"), "{again}");
    let edge = Edge::start(home.path());
    let bearer = format!("Bearer {token}");
    let session = edge.open_session("copilot-cloud-agent", &bearer, None);
    let fetched = edge.in_session(
        "copilot-cloud-agent",
        &bearer,
        &session,
        &call(1, "context_fetch", json!({"url": origin.handle.url()})),
    );
    assert_eq!(
        body(&fetched)["result"]["isError"],
        false,
        "{}",
        text(&fetched)
    );
    assert_eq!(
        crossings(&records(home.path(), &session))[0]["payload"]["principal"],
        "token:ci:acme/site"
    );

    let (ok, listed, _) = run(&["list"]);
    assert!(ok);
    assert!(
        listed.contains("ci:acme/site") && listed.contains("standing"),
        "{listed}"
    );
    let (ok, _, _) = run(&["revoke", "ci:acme/site"]);
    assert!(ok);
    let (ok, listed, _) = run(&["list"]);
    assert!(ok);
    assert!(listed.contains("revoked"), "{listed}");

    // No restart: the running edge refuses the next request by name, so the
    // session it opened cannot be reached without the token.
    let refused = edge.in_session(
        "copilot-cloud-agent",
        &bearer,
        &session,
        &call(2, "context_fetch", json!({"url": origin.handle.url()})),
    );
    assert_eq!(refused.status, 401, "{}", text(&refused));
    let reason = body(&refused)["error_description"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        reason.contains("ci:acme/site") && reason.contains("revoked"),
        "{reason}"
    );
    assert_eq!(origin.hits.load(Ordering::SeqCst), 1);
    let (ok, _, twice) = run(&["revoke", "ci:acme/site"]);
    assert!(!ok);
    assert!(twice.contains("already revoked"), "{twice}");

    // A token issued with --host is accepted on that endpoint alone; the
    // refusal names the binding. An unbound token, as above, works on every
    // endpoint, and the record tells them apart only by the basis.
    let (ok, _, reason) = run(&["issue", "m365:operator", "--host", "claude-code"]);
    assert!(!ok);
    assert!(reason.contains("not an endpoint"), "{reason}");
    let (ok, bound, _) = run(&["issue", "m365:operator", "--host", "m365-copilot"]);
    assert!(ok);
    let (ok, listed, _) = run(&["list"]);
    assert!(ok);
    assert!(
        listed.contains("m365:operator") && listed.contains("/mcp/m365-copilot"),
        "{listed}"
    );
    let bound_bearer = format!("Bearer {bound}");
    let elsewhere = edge.post(
        "/mcp/chatgpt",
        &[("Authorization", &bound_bearer)],
        &initialize(None),
    );
    assert_eq!(elsewhere.status, 401, "{}", text(&elsewhere));
    let reason = body(&elsewhere)["error_description"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        reason.contains("m365:operator") && reason.contains("bound to the endpoint"),
        "{reason}"
    );
    // This is a fixture client, not a captured Microsoft client identity.
    let client = json!({"name": "m365-header-fixture", "version": "1"});
    let session = edge.open_session("m365-copilot", &bound_bearer, Some(client.clone()));
    let fetched = edge.in_session(
        "m365-copilot",
        &bound_bearer,
        &session,
        &call(1, "context_fetch", json!({"url": origin.handle.url()})),
    );
    assert_eq!(body(&fetched)["result"]["isError"], false);
    let crossing = crossings(&records(home.path(), &session))[0]["payload"].clone();
    assert_eq!(crossing["principal"], "token:m365:operator");
    assert_eq!(crossing["authentication_basis"], "edge_token");
    assert_eq!(crossing["host"], "m365-copilot");
    assert_eq!(crossing["client"], client);
    assert_eq!(crossing["content_hash"], payload(&fetched)["content_hash"]);
    assert_eq!(
        crossing["content_hash"],
        commonmeasure_types::canonical::sha256_digest(b"cloud agent")
    );
    assert_eq!(
        fetched.headers.get("Content-Type"),
        Some("application/json")
    );
    assert_eq!(issuer.jwks_reads.load(Ordering::SeqCst), 0);

    // API-key placement must still satisfy the existing bearer contract.
    for (header, value) in [
        ("X-API-Key", bound.as_str()),
        ("Authorization", bound.as_str()),
    ] {
        let refused = edge.post("/mcp/m365-copilot", &[(header, value)], &initialize(None));
        assert_eq!(refused.status, 401, "{}", text(&refused));
        assert!(text(&refused).to_lowercase().contains("bearer"));
    }
    let refused = edge.post(
        &format!("/mcp/m365-copilot?api_key={bound}"),
        &[],
        &initialize(None),
    );
    assert_eq!(refused.status, 401);
    assert!(text(&refused).contains("bearer token is required"));
    assert_eq!(origin.hits.load(Ordering::SeqCst), 2);
}

#[test]
fn the_hosted_edge_refuses_to_start_unenrolled_or_with_an_origin_that_is_not_one() {
    let home = tempfile::tempdir().expect("tempdir");
    let unenrolled = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args([
            "hosted",
            "serve",
            "--listen",
            "127.0.0.1:0",
            "--origin",
            ORIGIN,
        ])
        .env("COMMONMEASURE_HOME", home.path())
        .output()
        .expect("runs");
    assert!(!unenrolled.status.success());
    let reason = String::from_utf8_lossy(&unenrolled.stderr);
    assert!(reason.contains("enrolment record"), "{reason}");

    let issuer = Issuer::start();
    enrol(home.path(), &issuer);
    let with_path = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args([
            "hosted",
            "serve",
            "--listen",
            "127.0.0.1:0",
            "--origin",
            "https://edge.test/mcp",
        ])
        .env("COMMONMEASURE_HOME", home.path())
        .output()
        .expect("runs");
    assert!(!with_path.status.success());
    let reason = String::from_utf8_lossy(&with_path.stderr);
    assert!(reason.contains("no path"), "{reason}");

    // --host narrows the endpoints served; a word not served is an unknown
    // path, and the metadata is served for the words that are.
    let narrowed = Edge::start_with(home.path(), &["--host", "chatgpt"]);
    let served = narrowed.request(
        "GET",
        "/.well-known/oauth-protected-resource/mcp/chatgpt",
        &[],
        None,
    );
    assert_eq!(served.status, 200, "{}", text(&served));
    let unserved = narrowed.request(
        "GET",
        "/.well-known/oauth-protected-resource/mcp/claude-connector",
        &[],
        None,
    );
    assert_eq!(unserved.status, 404, "{}", text(&unserved));
    let unserved = narrowed.post("/mcp/claude-connector", &[], &initialize(None));
    assert_eq!(unserved.status, 404, "{}", text(&unserved));
    assert!(text(&unserved).contains("/mcp/chatgpt"));
    assert!(!text(&unserved).contains("/mcp/claude-connector,"));
}
