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
        Self::start_in(home, extra, |_| {})
    }

    /// As [`Self::start_with`], with `configure` given the command before it
    /// runs: an environment variable or a working directory.
    fn start_in(home: &Path, extra: &[&str], configure: impl FnOnce(&mut Command)) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_commonmeasure"));
        command
            .args([
                "hosted",
                "serve",
                "--listen",
                "127.0.0.1:0",
                "--origin",
                ORIGIN,
            ])
            .args(extra)
            .env("COMMONMEASURE_HOME", home);
        configure(&mut command);
        let mut child = command
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

/// Assert that `response` names no path under the operator's home, as the
/// test gave it or as the file system resolves it, in its body or in any
/// header: a hosted tenant can act on neither, so the session record is
/// named relative to the home and the operator's policy by no path
/// (`docs/contracts/session-evidence.md`).
fn assert_names_no_home(home: &Path, response: &Response) {
    let mut served = String::from_utf8_lossy(&response.body).into_owned();
    for (name, value) in response.headers.iter() {
        served.push_str(&format!("\n{name}: {value}"));
    }
    let resolved = home.canonicalize().expect("the home resolves");
    for form in [home, resolved.as_path()] {
        assert!(
            !served.contains(&form.display().to_string()),
            "a hosted tenant is given the operator's home {}: {served}",
            form.display()
        );
    }
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
/// the tools list with `readOnlyHint`, the fetch returns the text and the
/// hash of the extracted text, which here it returned whole, the crossing is
/// recorded under the token's subject with its basis, the client is
/// identified once, and the session id in the header is the one on every
/// record.
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
        payload(&enrol)["error"]
            == "unknown tool; the tools are context_fetch, context_search, context_status",
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
    assert_names_no_home(home.path(), &fetched);
    let result = payload(&fetched);
    assert_eq!(result["content"], "Ofgem sets the cap quarterly.");
    assert_eq!(
        result["recorded_in"],
        format!("sessions/{session}.ndjson"),
        "a hosted fetch names its session record relative to the operator's home"
    );
    assert_eq!(result["http_status"], 200);
    let content_hash = result["content_hash"].as_str().expect("hash").to_owned();
    assert!(content_hash.starts_with("sha256:"));
    assert_eq!(
        content_hash,
        commonmeasure_types::canonical::sha256_digest(b"Ofgem sets the cap quarterly."),
        "the agent is told the hash of the whole text, which here is all it received"
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
    assert_names_no_home(home.path(), &status);
    let status = payload(&status);
    assert_eq!(status["session_id"], session);
    assert_eq!(status["client"], client);
    assert_eq!(status["evidence"], format!("sessions/{session}.ndjson"));
    assert_eq!(status["policy"]["source"], "policy.json");
    assert_eq!(status["credentials"]["path"], "credentials.env");
}

/// A session that cannot be opened is answered `500`, and the reason names
/// the operator's files relative to its home: the tenant can act on none.
#[test]
fn a_session_that_cannot_be_opened_names_no_operator_path() {
    let issuer = Issuer::start();
    let home = tempfile::tempdir().expect("tempdir");
    enrol(home.path(), &issuer);
    write_policy(home.path(), r#"{"policy_mode":"strict"}"#);
    let edge = Edge::start(home.path());
    let authorization = issuer.bearer("user-1", "claude-connector");
    let session = edge.open_session("claude-connector", &authorization, None);
    // The session is opened at its first request that is not lifecycle,
    // after the policy stopped parsing.
    std::fs::write(home.path().join("policy.json"), "{").expect("policy");

    let refused = edge.in_session(
        "claude-connector",
        &authorization,
        &session,
        &call(1, "context_status", json!({})),
    );
    assert_eq!(refused.status, 500, "{}", text(&refused));
    let error = body(&refused)["error"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        error.starts_with("the session could not be opened: policy.json "),
        "the policy is named relative to the home, with no separator before it: {error}"
    );
    assert_names_no_home(home.path(), &refused);
}

/// `hosted serve` does not hold the private-address floor, so a private
/// address the policy does not admit is refused with the setting that would
/// admit it, and the policy is named by no path (review R5). The boundary
/// rewrite would turn a full path into `policy.json`, so the producer's own
/// wording is asserted too. Nothing is sent: the address is refused before
/// any request.
#[test]
fn a_private_address_refusal_names_the_operators_policy_by_no_path() {
    let issuer = Issuer::start();
    let home = tempfile::tempdir().expect("tempdir");
    enrol(home.path(), &issuer);
    write_policy(home.path(), r#"{"policy_mode":"strict"}"#);
    let edge = Edge::start(home.path());
    let authorization = issuer.bearer("user-1", "claude-connector");
    let session = edge.open_session("claude-connector", &authorization, None);

    let refused = edge.in_session(
        "claude-connector",
        &authorization,
        &session,
        &call(1, "context_fetch", json!({"url": "http://10.0.0.1/"})),
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
    assert!(
        detail.contains("\"allow_private_hosts\": true, in the operator's policy if"),
        "{detail}"
    );
    assert_names_no_home(home.path(), &refused);
}

/// A home written with a trailing slash, as a unit file's `Environment=`
/// often gives it, is matched as the same home (review R3).
#[test]
fn a_home_given_with_a_trailing_slash_is_named_by_no_path() {
    let issuer = Issuer::start();
    let home = tempfile::tempdir().expect("tempdir");
    enrol(home.path(), &issuer);
    write_policy(home.path(), r#"{"policy_mode":"strict"}"#);
    let given = format!("{}/", home.path().display());
    let edge = Edge::start(Path::new(&given));
    let authorization = issuer.bearer("user-1", "claude-connector");
    let session = edge.open_session("claude-connector", &authorization, None);
    std::fs::write(home.path().join("policy.json"), "{").expect("policy");

    let refused = edge.in_session(
        "claude-connector",
        &authorization,
        &session,
        &call(1, "context_status", json!({})),
    );
    assert_eq!(refused.status, 500, "{}", text(&refused));
    let error = body(&refused)["error"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        error.starts_with("the session could not be opened: policy.json "),
        "{error}"
    );
    assert_names_no_home(home.path(), &refused);
}

/// A token file the edge cannot parse is met by the first caller presenting
/// an edge token, before anything authenticates it: the 401 names the file,
/// in its body and its challenge, and not where the operator keeps it
/// (review R1.1).
#[test]
fn an_unreadable_token_file_is_named_by_no_path_before_authentication() {
    let issuer = Issuer::start();
    let home = tempfile::tempdir().expect("tempdir");
    enrol(home.path(), &issuer);
    std::fs::write(home.path().join("hosted-tokens.json"), "{").expect("token file");
    let edge = Edge::start(home.path());

    let refused = edge.post(
        "/mcp/claude-connector",
        &[("Authorization", "Bearer cmet_anything")],
        &initialize(None),
    );
    assert_eq!(refused.status, 401, "{}", text(&refused));
    let description = body(&refused)["error_description"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        description.starts_with("hosted-tokens.json is not a valid token file"),
        "{description}"
    );
    let challenge = refused
        .headers
        .get("WWW-Authenticate")
        .expect("a challenge")
        .to_owned();
    assert!(
        challenge.contains("error_description=\"hosted-tokens.json is not a valid token file"),
        "{challenge}"
    );
    assert_names_no_home(home.path(), &refused);
}

/// An allowance ledger the edge cannot read leaves the remaining allowance
/// unknown. Observe fetches with the breach recorded and strict refuses;
/// either way the tenant is told the ledger relative to the home (review
/// R1.2). A subject binding carries the allowance, so no OS user is needed.
#[test]
fn an_unreadable_allowance_ledger_is_named_by_no_path_when_observed() {
    let (fetched, home) = fetch_against_an_unreadable_ledger("observe");
    assert_eq!(
        body(&fetched)["result"]["isError"],
        false,
        "{}",
        text(&fetched)
    );
    let result = payload(&fetched);
    for said in [&result["breach"], &result["allowance"]["reason"]] {
        let said = said.as_str().unwrap_or_default();
        assert!(
            said.contains("ledger could not be consulted: allowance/ledger.ndjson line 1"),
            "{result}"
        );
    }
    assert_names_no_home(home.path(), &fetched);
}

#[test]
fn an_unreadable_allowance_ledger_is_named_by_no_path_when_refused() {
    let (refused, home) = fetch_against_an_unreadable_ledger("strict");
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
    assert!(
        detail.contains("ledger could not be consulted: allowance/ledger.ndjson line 1")
            && detail.ends_with("(the operator's policy)"),
        "{detail}"
    );
    assert_names_no_home(home.path(), &refused);
}

/// One hosted fetch of a page whose licence quotes a price, by a subject
/// holding a day allowance, with the ledger holding a line that is not an
/// entry.
fn fetch_against_an_unreadable_ledger(mode: &str) -> (Response, tempfile::TempDir) {
    let home = tempfile::tempdir().expect("tempdir");
    let (fetched, _) =
        fetch_page_against_an_unreadable_ledger(mode, &home, "/article", "the priced article");
    (fetched, home)
}

/// As [`fetch_against_an_unreadable_ledger`], for `page` served at `target`.
/// Returns the answer and the URL asked for.
fn fetch_page_against_an_unreadable_ledger(
    mode: &str,
    home: &tempfile::TempDir,
    target: &str,
    page: &str,
) -> (Response, String) {
    const ROBOTS: &str = "License: /license.xml\nUser-agent: *\nAllow: /\n";
    const PRICED: &str = r#"<rsl xmlns="https://rslstandard.org/rsl">
  <content url="/"><license>
    <permits type="usage">ai-input</permits>
    <payment type="use"><amount currency="USD">0.015</amount></payment>
  </license></content></rsl>"#;
    let issuer = Issuer::start();
    let page = page.to_owned();
    let publisher = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |request| match request.target.as_str() {
            "/robots.txt" => Response::text(200, ROBOTS),
            "/license.xml" => Response::new(200, PRICED.as_bytes().to_vec()),
            "/.well-known/content-telemetry.json" => Response::text(404, "no manifest"),
            _ => Response::text(200, &page),
        })
        .expect("spawn");
    enrol(home.path(), &issuer);
    write_policy(
        home.path(),
        &json!({
            "policy_mode": mode,
            "allow_private_hosts": true,
            "principals": [{
                "principal": "capped", "subject": "user-1",
                "allowances": [{
                    "period": "day",
                    "amount": {"currency": "USD", "micros": 1_000_000},
                    "timezone": "UTC",
                }],
            }],
        })
        .to_string(),
    );
    std::fs::create_dir_all(home.path().join("allowance")).expect("allowance directory");
    std::fs::write(
        home.path().join("allowance/ledger.ndjson"),
        "invalid ledger\n",
    )
    .expect("ledger");
    let edge = Edge::start(home.path());
    let authorization = issuer.bearer("user-1", "claude-connector");
    let session = edge.open_session("claude-connector", &authorization, None);
    let url = format!("{}{target}", publisher.url());
    let fetched = edge.in_session(
        "claude-connector",
        &authorization,
        &session,
        &call(1, "context_fetch", json!({ "url": url })),
    );
    assert_eq!(fetched.status, 200, "{}", text(&fetched));
    (fetched, url)
}

/// A page that quotes the operator's home, fetched at a URL naming it, is
/// served as the publisher sent it: its content matches its hash and range,
/// its URL and the `next` built from it still name the page. The operator's
/// words in the same answer name the ledger relative to the home (review
/// F1; the review's probe).
#[test]
fn a_page_quoting_the_home_is_served_as_received_beside_operator_words_named_relative() {
    let home = tempfile::tempdir().expect("tempdir");
    let given = home.path().display().to_string();
    let resolved = home.path().canonicalize().expect("resolves");
    let resolved = resolved.display().to_string();
    let quoting = format!(
        "config: {given}/policy.json\nresolved: {resolved}/policy.json\nhome: {given}\n\
         {given}/allowance/ledger.ndjson\ndecoy: /srv/decoy/policy.json\n"
    );
    let page: String = quoting
        .chars()
        .chain("Ofgem sets the cap every quarter. ".chars().cycle())
        .take(70_000)
        .collect();
    let target = format!("/page?p={given}/x");
    let (fetched, url) = fetch_page_against_an_unreadable_ledger("observe", &home, &target, &page);
    assert_eq!(
        body(&fetched)["result"]["isError"],
        false,
        "{}",
        text(&fetched)
    );
    let result = payload(&fetched);

    let head: String = page.chars().take(60_000).collect();
    assert_eq!(result["content"], head.as_str());
    assert_eq!(
        result["content_hash"],
        commonmeasure_types::canonical::sha256_digest(page.as_bytes())
    );
    assert_eq!(
        result["content_range"],
        json!({"offset": 0, "chars": 60_000, "total_chars": 70_000})
    );
    assert_eq!(result["url"], url.as_str());
    // One answer carries one version of the asked URL (review G3).
    assert_eq!(
        result["declarations"]["robots"]["requested_url"],
        url.as_str()
    );
    assert_eq!(
        result["next"],
        format!(
            "More text follows: call context_fetch with url {url} and offset 60000 for the \
             next part. Each part is a new request to the site."
        )
        .as_str()
    );
    for said in [&result["breach"], &result["allowance"]["reason"]] {
        let said = said.as_str().unwrap_or_default();
        assert!(
            said.contains("ledger could not be consulted: allowance/ledger.ndjson line 1"),
            "{said}"
        );
        assert!(
            !said.contains(&given) && !said.contains(&resolved),
            "{said}"
        );
    }
    assert!(
        !result["recorded_in"]
            .as_str()
            .unwrap_or_default()
            .contains(&given),
        "{result}"
    );
}

/// A search over the operator's internal corpus, kept under the home, whose
/// document quotes the home: the result's title and text are served as the
/// corpus holds them, and its `file://` URL whole (review F1).
#[test]
fn an_internal_corpus_document_quoting_the_home_is_served_as_received() {
    let issuer = Issuer::start();
    let home = tempfile::tempdir().expect("tempdir");
    let given = home.path().display().to_string();
    let corpus = home.path().join("corpus");
    std::fs::create_dir_all(&corpus).expect("corpus");
    std::fs::write(
        corpus.join("corpus.json"),
        r#"{"name": "operator runbooks",
            "licence": {"state": "declared", "reference": "test/kb-licence-v1"}}"#,
    )
    .expect("manifest");
    let document =
        format!("# Deploying {given}\n\nThe gateway reads {given}/policy.json at start.\n");
    std::fs::write(corpus.join("deploy.md"), &document).expect("document");
    let canonical = corpus.canonicalize().expect("canonical corpus");
    enrol(home.path(), &issuer);
    write_policy(
        home.path(),
        &json!({
            "policy_mode": "observe",
            "record_internal_prefixes": [format!("file://{}/", canonical.display())],
        })
        .to_string(),
    );
    let edge = Edge::start_in(home.path(), &[], |command| {
        command.env("COMMONMEASURE_INTERNAL_CORPUS", &corpus);
    });
    let authorization = issuer.bearer("user-1", "claude-connector");
    let session = edge.open_session("claude-connector", &authorization, None);
    let searched = edge.in_session(
        "claude-connector",
        &authorization,
        &session,
        &call(
            1,
            "context_search",
            json!({"query": "gateway policy", "provider": "internal"}),
        ),
    );
    assert_eq!(searched.status, 200, "{}", text(&searched));
    let result = payload(&searched);
    let found = &result["results"][0];
    assert_eq!(found["text"], document.as_str(), "{result}");
    assert_eq!(found["title"], format!("Deploying {given}").as_str());
    assert!(
        found["url"]
            .as_str()
            .is_some_and(|url| url.starts_with(&format!("file://{}/", canonical.display()))),
        "{result}"
    );
    assert!(
        !result["recorded_in"]
            .as_str()
            .unwrap_or_default()
            .contains(&given),
        "{result}"
    );
}

/// A home given relative to the working directory is matched as the
/// absolute path it names, so a word that spells it is not rewritten: a
/// home `cm` leaves an offset `"cm"` quoted as it is (review F2).
#[test]
fn a_relative_home_leaves_a_word_that_spells_it() {
    let issuer = Issuer::start();
    let directory = tempfile::tempdir().expect("tempdir");
    let home = directory.path().join("cm");
    std::fs::create_dir_all(&home).expect("home");
    enrol(&home, &issuer);
    write_policy(&home, r#"{"policy_mode":"strict"}"#);
    let edge = Edge::start_in(Path::new("cm"), &[], |command| {
        command.current_dir(directory.path());
    });
    let authorization = issuer.bearer("user-1", "claude-connector");
    let session = edge.open_session("claude-connector", &authorization, None);

    let answered = edge.in_session(
        "claude-connector",
        &authorization,
        &session,
        &call(
            1,
            "context_fetch",
            json!({"url": "https://publisher.test/", "offset": "cm"}),
        ),
    );
    assert_eq!(answered.status, 200, "{}", text(&answered));
    assert_eq!(
        payload(&answered)["error"],
        "offset must be a non-negative integer, got \"cm\""
    );
    assert_names_no_home(&home, &answered);
}

/// A tool name, a protocol version and a JSON-RPC method are not quoted
/// back, so a tenant who puts a guessed home in any of them learns nothing
/// from the rewrite of the answer (review G2, H3).
#[test]
fn a_tool_name_a_protocol_version_or_a_method_naming_the_home_is_not_quoted_back() {
    let issuer = Issuer::start();
    let home = tempfile::tempdir().expect("tempdir");
    enrol(home.path(), &issuer);
    write_policy(home.path(), r#"{"policy_mode":"strict"}"#);
    let edge = Edge::start(home.path());
    let authorization = issuer.bearer("user-1", "claude-connector");
    let session = edge.open_session("claude-connector", &authorization, None);
    let guess = format!("{}/x", home.path().display());

    let named = edge.in_session(
        "claude-connector",
        &authorization,
        &session,
        &call(1, &guess, json!({})),
    );
    assert_eq!(named.status, 200, "{}", text(&named));
    assert_eq!(
        payload(&named)["error"],
        "unknown tool; the tools are context_fetch, context_search, context_status"
    );
    assert_names_no_home(home.path(), &named);

    let versioned = edge.post(
        "/mcp/claude-connector",
        &[
            ("Authorization", authorization.as_str()),
            ("Mcp-Session-Id", session.as_str()),
            ("MCP-Protocol-Version", guess.as_str()),
        ],
        &json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}),
    );
    assert_eq!(versioned.status, 400, "{}", text(&versioned));
    assert_eq!(
        body(&versioned)["error"],
        "MCP-Protocol-Version is not a revision this edge serves; it serves 2025-06-18, 2025-11-25"
    );
    assert_names_no_home(home.path(), &versioned);

    // Nor is a JSON-RPC method (review H3).
    let method = edge.in_session(
        "claude-connector",
        &authorization,
        &session,
        &json!({"jsonrpc": "2.0", "id": 3, "method": guess}),
    );
    assert_eq!(method.status, 200, "{}", text(&method));
    assert_eq!(body(&method)["error"]["code"], -32601, "{}", text(&method));
    assert_eq!(body(&method)["error"]["message"], "method not found");
    assert_names_no_home(home.path(), &method);
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
    assert!(
        detail.ends_with("(the operator's policy)"),
        "a hosted refusal names the operator's policy by no path: {detail}"
    );
    assert_names_no_home(home.path(), &refused);
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

/// The hosted edge bounds a fetch as the stdio edge does: the first part is
/// 60,000 characters, and the next is a second request and a second crossing.
#[test]
fn a_hosted_fetch_over_the_bound_arrives_in_parts_each_a_crossing() {
    let issuer = Issuer::start();
    let text: String = "Ofgem sets the cap every quarter. "
        .chars()
        .cycle()
        .take(70_000)
        .collect();
    let origin = origin(Box::leak(text.clone().into_boxed_str()));
    let home = tempfile::tempdir().expect("tempdir");
    enrol(home.path(), &issuer);
    write_policy(
        home.path(),
        r#"{"policy_mode":"strict","allow_private_hosts":true}"#,
    );
    let edge = Edge::start(home.path());
    let authorization = issuer.bearer("user-1", "claude-connector");
    let session = edge.open_session("claude-connector", &authorization, None);

    let first = payload(&edge.in_session(
        "claude-connector",
        &authorization,
        &session,
        &call(1, "context_fetch", json!({"url": origin.handle.url()})),
    ));
    let second = payload(&edge.in_session(
        "claude-connector",
        &authorization,
        &session,
        &call(
            2,
            "context_fetch",
            json!({"url": origin.handle.url(), "offset": 60_000}),
        ),
    ));
    assert_eq!(
        first["content_range"],
        json!({"offset": 0, "chars": 60_000, "total_chars": 70_000})
    );
    assert_eq!(first["truncated"], true);
    assert_eq!(second["truncated"], false);
    let head: String = text.chars().take(60_000).collect();
    let tail: String = text.chars().skip(60_000).collect();
    assert_eq!(first["content"], head.as_str());
    assert_eq!(second["content"], tail.as_str());
    assert_eq!(origin.hits.load(Ordering::SeqCst), 2);

    let recorded = records(home.path(), &session);
    let crossings = crossings(&recorded);
    assert_eq!(crossings.len(), 2);
    for (crossing, part) in crossings.iter().zip([&head, &tail]) {
        assert_eq!(
            crossing["payload"]["delivered"]["hash"],
            commonmeasure_types::canonical::sha256_digest(part.as_bytes())
        );
        assert_eq!(
            crossing["payload"]["content_hash"],
            commonmeasure_types::canonical::sha256_digest(text.as_bytes())
        );
    }
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
    // The delay's own text names no store path, and the suffix names the
    // session record relative to the operator's home.
    assert!(
        !detail.contains("crawl-delay"),
        "the refusal names the operator's store: {detail}"
    );
    let error = payload(&refused)["error"]
        .as_str()
        .expect("an error")
        .to_owned();
    assert!(
        error.ends_with(&format!(
            "The refusal is recorded in sessions/{session}.ndjson."
        )),
        "{error}"
    );
    assert_names_no_home(home.path(), &refused);

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

    // A protocol version this edge does not serve is 400, naming the
    // revisions it serves and not the one sent.
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
    assert!(text(&old).contains("2025-06-18, 2025-11-25"));
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
    assert!(!text(&batching).contains("2025-03-26"));
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

/// The `Origin` check runs before the token is read, so its refusal quotes
/// only a canonical origin, which has no path: an `Origin` naming a path
/// under the home, or any other path, is refused with no value, and cannot
/// confirm a guessed home to a caller holding no token (review G1).
#[test]
fn an_origin_that_is_not_an_origin_is_refused_quoting_nothing_before_authentication() {
    let issuer = Issuer::start();
    let home = tempfile::tempdir().expect("tempdir");
    enrol(home.path(), &issuer);
    let edge = Edge::start(home.path());
    let given = home.path().display().to_string();
    for presented in [format!("{given}/x"), "/srv/wrong/x".to_owned()] {
        let refused = edge.post(
            "/mcp/chatgpt",
            &[("Origin", presented.as_str())],
            &initialize(None),
        );
        assert_eq!(refused.status, 403, "{}", text(&refused));
        let served = text(&refused);
        assert!(
            body(&refused)["error"]
                .as_str()
                .is_some_and(|error| error.starts_with("the Origin header is not an origin")),
            "{served}"
        );
        for quoted in ["\\\"x\\\"", "/x", "wrong"] {
            assert!(!served.contains(quoted), "{presented}: {served}");
        }
        assert_names_no_home(home.path(), &refused);
    }

    // An origin whose path collapses to `/` through dot-segments is an
    // origin, and is quoted in canonical form, never as sent: as sent it
    // would carry the home into the rewrite (review H2).
    let segments = home.path().components().count();
    let collapsing = format!("https://h.test{given}/x/{}", "../".repeat(segments + 1));
    let refused = edge.post(
        "/mcp/chatgpt",
        &[("Origin", collapsing.as_str())],
        &initialize(None),
    );
    assert_eq!(refused.status, 403, "{}", text(&refused));
    let error = body(&refused)["error"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        error.starts_with("Origin https://h.test is not this edge's origin"),
        "{error}"
    );
    assert!(!text(&refused).contains("/x"), "{}", text(&refused));
    assert_names_no_home(home.path(), &refused);
}

/// The token checks that run before the signature is verified quote
/// nothing from the token: a forged `alg`, `iss` or `kid` naming a path
/// under the home cannot confirm a guessed home to a caller holding no
/// valid token (review H1). The last segment is distinctive so that its
/// absence is meaningful in prose that holds the letter `x`.
#[test]
fn a_forged_token_claim_is_not_quoted_before_the_signature_is_checked() {
    let issuer = Issuer::start();
    let home = tempfile::tempdir().expect("tempdir");
    enrol(home.path(), &issuer);
    let edge = Edge::start(home.path());
    let guess = format!("{}/xq7", home.path().display());
    let good = issuer.claims("user-1", "claude-connector");
    let mut forged_issuer = good.clone();
    forged_issuer["iss"] = json!(guess);
    let cases = [
        (
            "alg",
            issuer.token_with(&json!({"alg": guess, "kid": KEY_ID}), &good, &issuer.key),
        ),
        ("iss", issuer.token(&forged_issuer)),
        (
            "kid",
            issuer.token_with(&json!({"alg": "EdDSA", "kid": guess}), &good, &issuer.key),
        ),
    ];
    for (claim, token) in &cases {
        let refused = edge.post(
            "/mcp/claude-connector",
            &[("Authorization", &format!("Bearer {token}"))],
            &initialize(None),
        );
        assert_eq!(refused.status, 401, "{claim}: {}", text(&refused));
        let challenge = refused
            .headers
            .get("WWW-Authenticate")
            .unwrap_or_else(|| panic!("{claim}: a challenge"));
        for served in [text(&refused), challenge.to_owned()] {
            assert!(!served.contains("xq7"), "{claim}: {served}");
        }
        assert_names_no_home(home.path(), &refused);
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
        (
            "wrong issuer",
            issuer.token(&wrong_issuer),
            "the token's issuer is not the issuer pinned at enrolment",
        ),
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
            "signature does not verify under the key it names",
        ),
        (
            "a key id the issuer does not publish",
            issuer.token_with(
                &json!({"alg": "EdDSA", "kid": "hub-key-9"}),
                &good,
                &issuer.key,
            ),
            "the token's signing key is not one the issuer",
        ),
        (
            "an algorithm that is not EdDSA",
            issuer.token_with(&json!({"alg": "RS256", "kid": KEY_ID}), &good, &issuer.key),
            "the token's algorithm is not accepted; the hub signs EdDSA",
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
        // Before the signature is checked, nothing the token carries is
        // quoted back (review H1).
        for echoed in ["hub-key-9", "RS256", "other-hub.test"] {
            assert!(!text(&response).contains(echoed), "{case}: {echoed}");
            assert!(!challenge.contains(echoed), "{case}: {echoed}");
        }
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

// Catches: a hosted edge that starts for a stored hub URL carrying
// credentials, which 0.4.1's `connect` accepted. It pins that URL as its
// token issuer, so it would publish the key in the resource metadata, which
// needs no token, and in every 401, and would refuse every genuine token.
// The stored hub is loopback http, which passes the transport rule as https
// does, so only the credentials refuse it.
#[test]
fn the_hosted_edge_refuses_to_start_for_a_stored_hub_url_with_credentials() {
    let issuer = Issuer::start();
    let home = tempfile::tempdir().expect("tempdir");
    enrol(home.path(), &issuer);
    let path = home.path().join("enrolment.json");
    let mut enrolment: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("record")).expect("record JSON");
    enrolment["hub"] = json!(issuer.url().replace("http://", "http://ops:ak_PLANTED@"));
    std::fs::write(&path, enrolment.to_string()).expect("record written");

    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args([
            "hosted",
            "serve",
            "--listen",
            "127.0.0.1:0",
            "--origin",
            ORIGIN,
        ])
        .env("COMMONMEASURE_HOME", home.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary should start");
    // A start that is not refused announces its address; what it then serves
    // is the failure this test reports.
    let stdout = child.stdout.take().expect("stdout");
    let (lines, announced) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = lines.send(line);
        }
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut printed = Vec::new();
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait") {
            break status;
        }
        while let Ok(line) = announced.try_recv() {
            if let Some(address) = line.strip_prefix("listening on ") {
                let edge = Edge {
                    child,
                    base: address.trim().to_owned(),
                };
                let metadata = edge.request(
                    "GET",
                    "/.well-known/oauth-protected-resource/mcp/claude-connector",
                    &[],
                    None,
                );
                panic!(
                    "the hosted edge started for a hub URL with credentials and served {} {}",
                    metadata.status,
                    text(&metadata)
                );
            }
            printed.push(line);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "hosted serve neither started nor exited"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert!(!status.success());
    std::thread::sleep(std::time::Duration::from_millis(50));
    printed.extend(announced.try_iter());
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut child.stderr.take().expect("stderr"), &mut stderr)
        .expect("stderr");
    let output = format!("{}\n{stderr}", printed.join("\n"));
    assert!(
        stderr.contains(&format!(
            "the enrolled hub URL at {} carries credentials, a query or a fragment, so nothing \
             is sent to it",
            issuer.url()
        )) && stderr.contains("with the hub's address alone"),
        "{output}"
    );
    assert!(!output.contains("ak_PLANTED"), "{output}");
    assert_eq!(issuer.jwks_reads.load(Ordering::SeqCst), 0);
}
