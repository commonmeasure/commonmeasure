//! The one line the hub's Enrol this machine card prints, run as an operator
//! runs it: `sh -s -- --connect HUB --token TOKEN`, or `--connect HUB`
//! alone for the hub's device-code flow, with the installer on
//! stdin, as `curl … | sh` feeds it, in a new session with no controlling
//! terminal. Three loopback origins stand in for the world. The release
//! answers as `installer.rs`'s does, with this build's binary. The hub
//! answers the routes the first run calls (enrolment exchange, the
//! device-code request and poll, enrolment status,
//! the policy signer, the desired policy as a 404 or an envelope signed with
//! a real Ed25519 key, telemetry events) in the shapes `connect_e2e.rs`
//! copies from the hub's handlers. The publisher serves a page whose RSL
//! licence demands telemetry reporting, under a public name the debug
//! binary resolves to loopback (`COMMONMEASURE_TEST_HOSTS`).
//!
//! Every run clears the environment and gives the installer and the binary
//! scratch `HOME`, `COMMONMEASURE_HOME`, `CLAUDE_CONFIG_DIR` and
//! `CODEX_HOME`. The debug build's first-run page variable points the fetch
//! at the fixture page, which a release build cannot do; the session's host
//! word is the one a release build records (`first_run::session_host`), with
//! no override set. Everything else is the
//! production path: the installer, the binary it places, the registration
//! `install <host>` writes, the enrolment `connect --managed` makes, the
//! server `commonmeasure mcp` runs and the relay `commonmeasure relay` runs.
//!
//! The background relay is either a lock held as a running loop holds it
//! ([`Machine::background_relay`]) or, where a test gives the machine
//! LaunchAgents ([`Machine::with_launch_agents`]), the service the first run
//! installs: a debug build then takes the LaunchAgent path on any platform,
//! and a stand-in `launchctl` on `PATH` starts the program the written plist
//! names, so the loop that holds the home is this build's own `relay
//! --every`. That establishes the step order and the ruling with a real
//! loop holding the home, not launchd's behaviour.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use commonmeasure_harness::managed::{ENVELOPE, encode_hex};
use commonmeasure_http::{Request, Response, Server, ServerHandle};
use commonmeasure_types::canonical::{canonical_digest, canonical_json};
use ring::signature::KeyPair;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const REPOSITORY: &str = "commonmeasure/commonmeasure";

/// The token the hub's card mints in these tests. It reads as a marker: no
/// line of output and no file a run leaves may hold it.
const TOKEN: &str = "et_FIRSTRUNMARKER_5b1d9e";
/// A token the hub does not know. The hub's refusal quotes it back, as an
/// error body may, so the run's redaction is exercised too.
const UNKNOWN_TOKEN: &str = "et_UNKNOWNMARKER_c4a7f0";
const API_KEY: &str = "ak_first_run_ingest";
const ORGANISATION: &str = "11111111-1111-1111-1111-111111111111";

/// A name the ruling reads as public, which the debug binary resolves to
/// the publisher's loopback address.
const PUBLIC_NAME: &str = "publisher.test";

/// The page's licence: AI input permitted, each use reported by telemetry.
/// The endpoint is the source's own; an edge meets the demand through the
/// hub it is enrolled with (`docs/contracts/session-evidence.md`).
const REPORTING_LICENCE: &str = r#"<rsl xmlns="https://rslstandard.org/rsl">
  <content url="/"><license>
    <permits type="usage">ai-input</permits>
    <payment type="attribution"/>
    <reporting type="telemetry" profile="https://contenttelemetry.org/profiles/spur"
               endpoint="https://telemetry.publisher.test/events">
      <![CDATA[{"conformance_level": "grounding", "privacy_level": "minimal"}]]>
    </reporting>
  </license></content></rsl>"#;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/commonmeasure-cli sits two levels below the repository root")
        .to_path_buf()
}

fn asset_for_this_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "commonmeasure-linux-x64",
        ("linux", "aarch64") => "commonmeasure-linux-arm64",
        ("macos", "aarch64") => "commonmeasure-darwin-arm64",
        ("macos", "x86_64") => "commonmeasure-darwin-x64",
        other => panic!("the release has no binary for {other:?}"),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn redirect(to: &str) -> Response {
    let mut response = Response::new(302, Vec::new());
    response.headers.set("Location", to);
    response
}

/// The latest release, holding this build's binary for this platform, and
/// every path it was asked for.
fn release() -> (ServerHandle, Arc<Mutex<Vec<String>>>) {
    // One copy of the debug binary, which is large, for every release.
    static BINARY: std::sync::OnceLock<Arc<Vec<u8>>> = std::sync::OnceLock::new();
    let binary = Arc::clone(BINARY.get_or_init(|| {
        Arc::new(
            std::fs::read(env!("CARGO_BIN_EXE_commonmeasure"))
                .expect("the built binary is readable"),
        )
    }));
    let asset = asset_for_this_platform();
    let sums = format!("{}  {asset}\n", sha256_hex(&binary));
    let tag = format!("v{VERSION}");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&requests);
    let server = Server::bind("127.0.0.1:0").expect("bind");
    let origin = format!("http://{}", server.local_addr().expect("local addr"));
    let releases = format!("/{REPOSITORY}/releases");
    let handle = server
        .spawn(move |request: Request| {
            let path = request.target.split('?').next().unwrap_or_default();
            log.lock().unwrap().push(path.to_owned());
            if path == format!("{releases}/latest") {
                redirect(&format!("{origin}{releases}/tag/{tag}"))
            } else if let Some(rest) = path.strip_prefix(&format!("{releases}/download/")) {
                redirect(&format!("{origin}/objects/{rest}"))
            } else if path == format!("/objects/{tag}/SHA256SUMS") {
                Response::new(200, sums.clone().into_bytes())
            } else if path == format!("/objects/{tag}/{asset}") {
                Response::new(200, binary.to_vec())
            } else {
                Response::text(404, "no such route")
            }
        })
        .expect("spawn");
    (handle, requests)
}

/// The hub's policy signer: a real key pair, and the envelope it signs.
struct Signer(ring::signature::Ed25519KeyPair);

impl Signer {
    fn new() -> Self {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).expect("a key pair");
        Self(ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("pkcs8"))
    }

    fn public_key_hex(&self) -> String {
        encode_hex(self.0.public_key().as_ref())
    }

    /// Revision 1 of the organisation's policy, in observe mode, as the hub
    /// publishes it at organisation creation.
    fn revision_one(&self) -> Vec<u8> {
        let policy = json!({"policy_mode": "observe"});
        let payload = json!({
            "organisation": ORGANISATION,
            "edge_key_id": null,
            "revision": 1,
            "issued_at": "2026-10-01T00:00:00Z",
            "expires_at": "2099-01-01T00:00:00Z",
            "policy": policy,
            "digest": canonical_digest(&policy),
        });
        let signature = self.0.sign(canonical_json(&payload).as_bytes());
        serde_json::to_vec(&json!({
            "envelope": ENVELOPE,
            "payload": payload,
            "signature": {
                "algorithm": "ed25519",
                "key_id": "hub-policy-test",
                "value": encode_hex(signature.as_ref()),
            },
        }))
        .expect("serialises")
    }
}

/// The device code the loopback hub issues. It reads as a marker: no line
/// of output and no file a run leaves may hold it.
const DEVICE_CODE: &str = "dc_DEVICEMARKER_7e2a41";
const USER_CODE: &str = "BCDF-GHJK";

/// What the loopback hub saw.
#[derive(Default)]
struct Hub {
    /// Every route asked, in order.
    routes: Vec<String>,
    /// Telemetry batches delivered, as posted.
    batches: Vec<Value>,
    key_id: String,
    /// When set, the device-code answer states this lifetime in seconds and
    /// every poll is answered pending, as for a code nobody approves.
    unapproved_lifetime: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EchoFailure {
    Status,
    Proof,
    Signer,
    Policy,
    Telemetry,
}

impl EchoFailure {
    fn route(self) -> (&'static str, &'static str) {
        match self {
            Self::Status => ("GET", "/api/v1/enrolment/status"),
            Self::Proof => ("PUT", "/api/v1/enrolment/directory-proof"),
            Self::Signer => ("GET", "/api/v1/policy/signer"),
            Self::Policy => ("GET", "/api/v1/policy/desired"),
            Self::Telemetry => ("POST", "/api/v1/telemetry/events"),
        }
    }
}

/// A hub that knows [`TOKEN`]. With `published`, its desired-policy route
/// serves revision 1 signed by its signer; without, it answers 404, as a
/// hub whose organisation has published no revision does.
fn hub(published: bool) -> (ServerHandle, Arc<Mutex<Hub>>) {
    hub_with_failure(published, None)
}

fn hub_with_failure(
    published: bool,
    failure: Option<EchoFailure>,
) -> (ServerHandle, Arc<Mutex<Hub>>) {
    let state = Arc::new(Mutex::new(Hub::default()));
    let seen = Arc::clone(&state);
    let signer = Signer::new();
    let public_key = signer.public_key_hex();
    let envelope = published.then(|| signer.revision_one());
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |request: Request| {
            let mut state = seen.lock().unwrap();
            state
                .routes
                .push(format!("{} {}", request.method, request.target));
            if failure.is_some_and(|failure| {
                failure.route() == (request.method.as_str(), request.target.as_str())
            }) {
                // Escape one character too: scanning only raw response bytes
                // would miss the token once JSON decoding restores it.
                let detail = json!({"detail": format!("refused credential from enrolment token {TOKEN}")});
                return Response::json(401, &detail.to_string().replace("et_", "\\u0065t_"));
            }
            let keyed = request.headers.get("X-API-Key") == Some(API_KEY);
            let refused = || Response::json(401, r#"{"detail":"a valid credential is required"}"#);
            match (request.method.as_str(), request.target.as_str()) {
                ("POST", "/api/v1/enrolment/exchange") => {
                    let body: Value = serde_json::from_slice(&request.body).unwrap();
                    let token = body["token"].as_str().unwrap_or_default();
                    if token != TOKEN {
                        let detail = json!({"detail": format!("enrolment token {token} is unknown")});
                        return Response::json(401, &detail.to_string());
                    }
                    enrolled(&mut state, &body, failure)
                }
                // The device-code route, approved by the time of the first
                // poll unless the hub was told otherwise, at the interval the
                // edge waits in these tests.
                ("POST", "/connect/device") => Response::json(
                    200,
                    &json!({
                        "device_code": DEVICE_CODE,
                        "user_code": USER_CODE,
                        "verification_uri": "https://hub.example/connect",
                        "verification_uri_complete": format!("https://hub.example/connect?code={USER_CODE}"),
                        "expires_in": state.unapproved_lifetime.unwrap_or(900),
                        "interval": 1,
                    })
                    .to_string(),
                ),
                ("POST", "/api/v1/enrolment/device/token") => {
                    let body: Value = serde_json::from_slice(&request.body).unwrap();
                    if body["device_code"] != json!(DEVICE_CODE) {
                        return Response::json(
                            400,
                            r#"{"error":"invalid_grant","error_description":"unknown device code"}"#,
                        );
                    }
                    if state.unapproved_lifetime.is_some() {
                        return Response::json(
                            400,
                            r#"{"error":"authorization_pending","error_description":"the code is not yet approved"}"#,
                        );
                    }
                    enrolled(&mut state, &body, failure)
                }
                ("GET", "/api/v1/enrolment/status") if keyed => Response::json(
                    200,
                    &json!({"key_id": state.key_id, "name": "laptop-7", "revoked_at": null, "revocation": null})
                        .to_string(),
                ),
                ("GET", "/api/v1/policy/signer") if keyed => Response::json(
                    200,
                    &json!({
                        "algorithm": "ed25519",
                        "key_id": "hub-policy-test",
                        "organisation": ORGANISATION,
                        "policy_path": "/api/v1/policy/desired",
                        "public_key": public_key,
                    })
                    .to_string(),
                ),
                ("GET", "/api/v1/policy/desired") => match &envelope {
                    Some(envelope) => {
                        let mut response = Response::new(200, envelope.clone());
                        response.headers.set("Content-Type", "application/json");
                        response
                    }
                    None => Response::json(
                        404,
                        r#"{"detail":"no policy revision has been published for this organisation"}"#,
                    ),
                },
                ("POST", "/api/v1/telemetry/events") if keyed => {
                    let body: Value = serde_json::from_slice(&request.body).unwrap();
                    let events = body["events"].as_array().map(Vec::len).unwrap_or(0);
                    state.batches.push(body);
                    Response::json(
                        201,
                        &json!({"status": "ok", "events_created": events}).to_string(),
                    )
                }
                ("GET" | "POST", "/api/v1/enrolment/status" | "/api/v1/policy/signer")
                | ("POST", "/api/v1/telemetry/events") => refused(),
                _ => Response::json(404, r#"{"detail":"not found"}"#),
            }
        })
        .expect("spawn");
    (handle, state)
}

/// The hub's `201` to an exchange or an approved poll, for the key `body`
/// presents.
fn enrolled(state: &mut Hub, body: &Value, failure: Option<EchoFailure>) -> Response {
    let x = body["public_key"]["x"].as_str().unwrap();
    let canonical = format!(r#"{{"crv":"Ed25519","kty":"OKP","x":"{x}"}}"#);
    let key_id = URL_SAFE_NO_PAD.encode(Sha256::digest(canonical.as_bytes()));
    state.key_id = key_id.clone();
    let mut answer = json!({
        "organization": {"id": ORGANISATION, "name": "Org A Media"},
        "name": "laptop-7",
        "key_id": key_id,
        "identity": {
            "origin": "https://hub.example",
            "bot_page": "https://hub.example/bot",
            "contact": "mailto:bot@hub.example",
        },
        "api_key_id": "22222222-2222-2222-2222-222222222222",
        "api_key": API_KEY,
        "telemetry_path": "/api/v1/telemetry",
    });
    if failure == Some(EchoFailure::Proof) {
        answer["directory_proof"] = json!({
            "authority": "hub.example",
            "lifetime_secs": 604_800,
            "expires_at": null,
        });
    }
    Response::json(201, &answer.to_string())
}

/// The first-run page's publisher, and how often the page itself was
/// requested (its `robots.txt` and licence are not counted).
fn publisher() -> (ServerHandle, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&hits);
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |request: Request| match request.target.as_str() {
            "/robots.txt" => {
                Response::text(200, "License: /license.xml\nUser-agent: *\nAllow: /\n")
            }
            "/license.xml" => Response::new(200, REPORTING_LICENCE.as_bytes().to_vec()),
            "/.well-known/content-telemetry.json" => Response::text(404, "no manifest"),
            _ => {
                count.fetch_add(1, Ordering::SeqCst);
                Response::text(200, "the first-run article")
            }
        })
        .expect("spawn");
    (handle, hits)
}

/// Where the runs' files go, the installer's own temporary copy of the
/// binary included: the target directory's scratch space, since each run
/// holds two copies of the debug binary and a small `/tmp` fills.
fn scratch() -> &'static Path {
    Path::new(env!("CARGO_TARGET_TMPDIR"))
}

/// The scratch machine a run happens on. Host configuration directories
/// exist only for the hosts a test says are present.
struct Machine {
    root: tempfile::TempDir,
    /// Set by [`Machine::with_launch_agents`].
    launch_agents: bool,
}

/// A stand-in for launchd that knows no service, loads every plist, and on
/// `kickstart` starts the program the loaded plist names, detached, as
/// launchd starts a RunAtLoad agent. Its state is kept beside it: `plist`,
/// `pid` and the loop's `relay.log`.
const LAUNCHCTL: &str = r#"#!/bin/sh
state=$(dirname "$0")
case "$1" in
  print)
    echo "Could not find service "$2" in domain for user gui: 501" >&2
    exit 113 ;;
  bootstrap)
    echo "$3" >"$state/plist"
    exit 0 ;;
  kickstart)
    set -- $(sed -n '/<key>ProgramArguments<\/key>/,/<\/array>/s/.*<string>\(.*\)<\/string>.*/\1/p' "$(cat "$state/plist")")
    "$@" </dev/null >>"$state/relay.log" 2>&1 &
    echo $! >"$state/pid"
    exit 0 ;;
esac
echo "launchctl stand-in: $* is not answered" >&2
exit 1
"#;

/// A stand-in for launchd whose `bootstrap` fails, as launchd's does for a
/// plist it will not load.
const LAUNCHCTL_REFUSING: &str = r#"#!/bin/sh
case "$1" in
  print)
    echo "Could not find service "$2" in domain for user gui: 501" >&2
    exit 113 ;;
  bootstrap)
    echo "Bootstrap failed: 5: Input/output error" >&2
    exit 5 ;;
esac
echo "launchctl stand-in: $* is not answered" >&2
exit 1
"#;

impl Machine {
    fn new() -> Self {
        let machine = Self {
            root: tempfile::tempdir_in(scratch()).expect("tempdir"),
            launch_agents: false,
        };
        std::fs::create_dir_all(machine.project()).unwrap();
        machine.install_opener();
        machine
    }

    /// Stand-ins for the desktop's openers, first on every run's `PATH`, so
    /// no test opens a browser. Each records the URL it was given in
    /// `opened` beside it and returns.
    fn install_opener(&self) {
        self.write_opener("#!/bin/sh\necho \"$1\" >>\"$(dirname \"$0\")/opened\"\n");
    }

    /// Openers that, having recorded the URL, stay in the foreground until
    /// `release` appears beside them, as `xdg-open` may for as long as the
    /// browser it started stays open. Each records `returned` as it exits,
    /// and gives up after a minute so that no test leaves one behind.
    fn with_held_opener(self) -> Self {
        self.write_opener(
            r#"#!/bin/sh
dir="$(dirname "$0")"
echo "$1" >>"$dir/opened"
i=0
while [ ! -e "$dir/release" ] && [ "$i" -lt 600 ]; do
  sleep 0.1
  i=$((i + 1))
done
echo "$1" >>"$dir/returned"
"#,
        );
        self
    }

    fn write_opener(&self, opener: &str) {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::create_dir_all(self.opener()).unwrap();
        for name in ["open", "xdg-open"] {
            let script = self.opener().join(name);
            std::fs::write(&script, opener).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn opener(&self) -> PathBuf {
        self.root.path().join("opener")
    }

    /// Whether a held opener was started and has not returned.
    fn opener_held(&self) -> bool {
        self.opener().join("opened").exists() && !self.opener().join("returned").exists()
    }

    /// Let a held opener return, and wait until it has.
    fn release_opener(&self) {
        std::fs::write(self.opener().join("release"), "").unwrap();
        let started = Instant::now();
        while !self.opener().join("returned").exists() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the held opener did not return"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The URLs the desktop's opener was given, in order.
    fn opened(&self) -> Vec<String> {
        std::fs::read_to_string(self.opener().join("opened"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The machine runs services as LaunchAgents, managed by `launchctl`, a
    /// script on the front of its `PATH`.
    fn with_launch_agents(mut self, launchctl: &str) -> Self {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::create_dir_all(self.launchd()).unwrap();
        let script = self.launchd().join("launchctl");
        std::fs::write(&script, launchctl).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        self.launch_agents = true;
        self
    }

    fn launchd(&self) -> PathBuf {
        self.root.path().join("launchd")
    }

    /// The relay service's plist, under the scratch label the environment
    /// sets.
    fn relay_plist(&self) -> PathBuf {
        self.home()
            .join("Library/LaunchAgents/ai.commonmeasure.test.first-run.relay.plist")
    }

    fn home(&self) -> PathBuf {
        self.root.path().join("home")
    }
    fn project(&self) -> PathBuf {
        self.home().join("site-2")
    }
    fn edge(&self) -> PathBuf {
        self.root.path().join("edge")
    }
    fn claude(&self) -> PathBuf {
        self.root.path().join("claude")
    }
    fn codex(&self) -> PathBuf {
        self.root.path().join("codex")
    }
    fn bin(&self) -> PathBuf {
        self.root.path().join("bin")
    }

    /// Claude Code and Codex as each leaves its directory on first run, with
    /// a setting of the operator's own in each.
    fn with_claude_and_codex(self) -> Self {
        std::fs::create_dir_all(self.claude()).unwrap();
        std::fs::write(self.claude().join("settings.json"), r#"{"model": "fable"}"#).unwrap();
        std::fs::create_dir_all(self.codex()).unwrap();
        std::fs::write(self.codex().join("config.toml"), "model = \"gpt-5\"\n").unwrap();
        self
    }

    /// Hold the background relay's lock on the edge home, as a running
    /// `commonmeasure relay --every` holds it, for as long as the file lives.
    fn background_relay(&self) -> std::fs::File {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(self.edge())
            .unwrap();
        let lock = std::fs::File::create(
            self.edge()
                .join(commonmeasure_harness::delivery::RELAY_LOCK_FILE),
        )
        .unwrap();
        lock.lock().unwrap();
        lock
    }

    /// The scratch environment, and nothing of this machine's but `PATH`.
    fn environment(&self, command: &mut Command) {
        command
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", self.opener().display()))
            .env("TMPDIR", scratch())
            .env("HOME", self.home())
            .env("COMMONMEASURE_HOME", self.edge())
            .env("CLAUDE_CONFIG_DIR", self.claude())
            .env("CODEX_HOME", self.codex())
            .env(
                "COMMONMEASURE_SERVICE_LABEL",
                "ai.commonmeasure.test.first-run",
            );
        if self.launch_agents {
            command
                .env(
                    "PATH",
                    format!(
                        "{}:{}:/usr/bin:/bin",
                        self.opener().display(),
                        self.launchd().display()
                    ),
                )
                .env("COMMONMEASURE_TEST_LAUNCH_AGENTS", "1");
        }
    }

    /// The installed binary, run in this machine's environment.
    fn commonmeasure(&self, cwd: &Path, args: &[&str]) -> Run {
        let mut command = Command::new(self.bin().join("commonmeasure"));
        self.environment(&mut command);
        command.args(args).current_dir(cwd).stdin(Stdio::null());
        Run::of(&mut command, None)
    }

    /// Every file under the scratch machine, by path, with its bytes.
    fn files(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        files_under(self.root.path())
    }
}

/// A relay loop the stand-in `launchctl` started outlives the run; it is
/// stopped with the machine, as `bootout` stops it.
impl Drop for Machine {
    fn drop(&mut self) {
        // A held opener a failing test left running returns too.
        let _ = std::fs::write(self.opener().join("release"), "");
        let Some(pid) = std::fs::read_to_string(self.launchd().join("pid"))
            .ok()
            .and_then(|pid| pid.trim().parse::<libc::pid_t>().ok())
        else {
            return;
        };
        // SAFETY: kill only sends a signal to the process the stand-in
        // started; it touches no memory of ours.
        unsafe { libc::kill(pid, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(10);
        // SAFETY: as above; signal 0 only asks whether the process exists.
        while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        // SAFETY: as above.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
}

fn files_under(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|error| panic!("scan {}: {error}", dir.display()));
    for entry in entries {
        let path = entry.unwrap().path();
        let kind = std::fs::symlink_metadata(&path).unwrap().file_type();
        if kind.is_dir() {
            files.extend(files_under(&path));
        } else if kind.is_file() {
            let bytes = std::fs::read(&path)
                .unwrap_or_else(|error| panic!("scan {}: {error}", path.display()));
            files.insert(path, bytes);
        }
    }
    files
}

/// What a run printed and how it ended.
struct Run {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

impl Run {
    /// Run `command` to its end, failing the test if it outlives a minute:
    /// a run that waits for an answer nobody gives hangs.
    fn of(command: &mut Command, stdin: Option<&[u8]>) -> Self {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        if stdin.is_some() {
            command.stdin(Stdio::piped());
        }
        let mut child = command.spawn().expect("the command starts");
        if let Some(bytes) = stdin {
            let mut pipe = child.stdin.take().unwrap();
            pipe.write_all(bytes).unwrap();
        }
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let out = std::thread::spawn(move || {
            let mut text = String::new();
            stdout.read_to_string(&mut text).unwrap();
            text
        });
        let err = std::thread::spawn(move || {
            let mut text = String::new();
            stderr.read_to_string(&mut text).unwrap();
            text
        });
        let deadline = Instant::now() + Duration::from_secs(60);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "the run outlived a minute: {}\n{}",
                    out.join().unwrap(),
                    err.join().unwrap()
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        Self {
            status,
            stdout: out.join().unwrap(),
            stderr: err.join().unwrap(),
        }
    }

    /// The one line a step printed, by its label.
    fn line(&self, label: &str) -> &str {
        let prefix = format!("{label}: ");
        let lines: Vec<&str> = self
            .stdout
            .lines()
            .filter(|line| line.starts_with(&prefix))
            .collect();
        assert_eq!(lines.len(), 1, "one `{label}` line:\n{}", self.both());
        lines[0]
    }

    fn both(&self) -> String {
        format!("stdout:\n{}\nstderr:\n{}", self.stdout, self.stderr)
    }
}

/// The world a run reaches: the release, the hub and the publisher.
struct World {
    release: ServerHandle,
    release_requests: Arc<Mutex<Vec<String>>>,
    hub: ServerHandle,
    hub_seen: Arc<Mutex<Hub>>,
    publisher: ServerHandle,
    page_hits: Arc<AtomicUsize>,
}

impl World {
    fn new(published: bool) -> Self {
        let (release, release_requests) = release();
        let (hub, hub_seen) = hub(published);
        let (publisher, page_hits) = publisher();
        Self {
            release,
            release_requests,
            hub,
            hub_seen,
            publisher,
            page_hits,
        }
    }

    fn page(&self) -> String {
        format!(
            "{}/article",
            self.publisher.url().replace("127.0.0.1", PUBLIC_NAME)
        )
    }

    /// `sh -s -- <args>` with the installer on stdin, as the card's line
    /// runs it, in a new session with no controlling terminal, from `cwd`.
    fn install(&self, machine: &Machine, cwd: &Path, args: &[&str]) -> Run {
        let mut command = self.installer(machine, cwd);
        command.args(["-s", "--"]).args(args);
        let script = std::fs::read(repo_root().join("install.sh")).unwrap();
        Run::of(&mut command, Some(&script))
    }

    fn installer(&self, machine: &Machine, cwd: &Path) -> Command {
        use std::os::unix::process::CommandExt as _;
        let mut command = Command::new("/bin/sh");
        machine.environment(&mut command);
        command
            .current_dir(cwd)
            .env(
                "COMMONMEASURE_RELEASE_URL",
                format!("{}/{REPOSITORY}/releases", self.release.url()),
            )
            .env(
                "COMMONMEASURE_TEST_HOSTS",
                format!("{PUBLIC_NAME}=127.0.0.1"),
            )
            .env("COMMONMEASURE_TEST_FIRST_RUN_PAGE", self.page());
        // SAFETY: setsid is async-signal-safe and changes only the child's
        // session, between fork and exec.
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        command
    }

    /// The card's line, with `extra` after it, into `machine`'s `--dir`.
    fn first_run(&self, machine: &Machine, cwd: &Path, token: &str, extra: &[&str]) -> Run {
        let bin = machine.bin();
        let hub = self.hub.url();
        let mut args = vec![
            "--dir",
            bin.to_str().unwrap(),
            "--connect",
            &hub,
            "--token",
            token,
        ];
        args.extend_from_slice(extra);
        self.install(machine, cwd, &args)
    }

    fn batches(&self) -> Vec<Value> {
        self.hub_seen.lock().unwrap().batches.clone()
    }
}

/// Every record of every session in `machine`'s edge home.
fn session_records(machine: &Machine) -> Vec<Value> {
    let mut records = Vec::new();
    for path in commonmeasure_harness::SessionLog::list(&machine.edge()).unwrap() {
        records.extend(commonmeasure_harness::SessionLog::read(&path).unwrap());
    }
    records
}

fn crossings(machine: &Machine) -> Vec<Value> {
    session_records(machine)
        .into_iter()
        .filter(|record| {
            matches!(
                record["event"].as_str(),
                Some("crossing_mediated" | "crossing_refused")
            )
        })
        .collect()
}

/// The host word the first run's own session is recorded under
/// (`docs/contracts/session-evidence.md` §Client identity).
const INSTALLER_WORD: &str = "commonmeasure-first-run";

/// Every record of the run's session, each crossing among them, names the
/// installer's word as its host, and the session's client is the first run
/// itself: no record names a registered host.
fn assert_recorded_under_the_installers_word(machine: &Machine) {
    let records = session_records(machine);
    let hosts: Vec<&Value> = records
        .iter()
        // A resolved manifest's `host` is the source's network host.
        .filter(|record| record["event"] != "manifest_resolved")
        .filter_map(|record| record["payload"].get("host"))
        .collect();
    assert!(!hosts.is_empty(), "{records:?}");
    assert!(
        hosts.iter().all(|host| *host == INSTALLER_WORD),
        "{hosts:?}"
    );
    let identified: Vec<&Value> = records
        .iter()
        .filter(|record| record["event"] == "client_identified")
        .collect();
    assert_eq!(identified.len(), 1, "{records:?}");
    assert_eq!(
        identified[0]["payload"]["client"]["name"], INSTALLER_WORD,
        "{}",
        identified[0]
    );
    for crossing in crossings(machine) {
        assert_eq!(crossing["payload"]["host"], INSTALLER_WORD, "{crossing}");
        assert_eq!(
            crossing["payload"]["client"]["name"], INSTALLER_WORD,
            "{crossing}"
        );
    }
}

/// The steps' lines, in the order the step table gives them.
const STEPS: [&str; 8] = [
    "hosts", "connect", "enrol", "service", "fetch", "relay", "evidence", "console",
];

fn assert_steps_in_order(run: &Run, steps: &[&str]) {
    let labels: Vec<&str> = run
        .stdout
        .lines()
        .filter_map(|line| line.split_once(": ").map(|(label, _)| label))
        .filter(|label| STEPS.contains(label))
        .collect();
    assert_eq!(labels, steps, "{}", run.both());
}

/// A machine with Claude Code and Codex, the hub's revision 1 published,
/// reporting consent agreed at install and a background relay holding the
/// edge home: every step's line as the step table gives it, one mediated
/// crossing in the first run's own session, and one delivery to the hub.
#[test]
fn the_line_takes_a_consenting_machine_to_a_delivered_first_crossing() {
    let world = World::new(true);
    let machine = Machine::new().with_claude_and_codex();
    let _relay = machine.background_relay();
    let run = world.first_run(&machine, &machine.project(), TOKEN, &["--agree-reporting"]);
    assert!(run.status.success(), "{}", run.both());
    assert_steps_in_order(&run, &STEPS);
    let hub = world.hub.url();
    let project = machine.project().canonicalize().unwrap();

    assert_eq!(
        run.line("hosts"),
        "hosts: registered claude-code, codex; not found: pi, claude-desktop, cursor, copilot-cli, vscode, chrome"
    );
    assert_eq!(
        run.line("connect"),
        format!(
            "connect: enrolled with {hub} in Org A Media as laptop-7; managed, policy revision 1 in force"
        )
    );
    assert_eq!(
        run.line("enrol"),
        format!(
            "enrol: nothing enrolled: hub reporting covers {0} and descendants, including existing \
             eligible witnessed evidence. Confirm with --include-history; related Git worktrees \
             need separate enrolment; to enrol it, run `commonmeasure enrol --name site-2 \
             --reporting hub --include-history` in {0}",
            project.display()
        )
    );
    assert_eq!(
        run.line("service"),
        "service: a background relay holds this edge home"
    );

    let crossings = crossings(&machine);
    assert_eq!(crossings.len(), 1, "{crossings:?}");
    let crossing = &crossings[0];
    assert_eq!(crossing["event"], "crossing_mediated", "{crossing}");
    let reporting = &crossing["payload"]["declarations"]["reporting"];
    assert_eq!(reporting["met"], true, "{reporting}");
    assert_eq!(reporting["route"], "hub", "{reporting}");
    assert_eq!(world.page_hits.load(Ordering::SeqCst), 1);
    let log = commonmeasure_harness::SessionLog::list(&machine.edge()).unwrap();
    assert_eq!(log.len(), 1, "the first run's own session: {log:?}");
    assert_recorded_under_the_installers_word(&machine);
    assert_eq!(
        run.line("fetch"),
        format!(
            "fetch: {} delivered, mediated; licence {}/license.xml, reported through the hub; recorded in {}",
            world.page(),
            world.page().trim_end_matches("/article"),
            log[0].display()
        )
    );

    let batches = world.batches();
    assert_eq!(batches.len(), 1, "one delivery: {batches:?}");
    // The hub receives the crossing under the installer's word, never as a
    // registered host's session.
    let host_tools: Vec<&Value> = batches[0]["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|event| event["data"].get("contextops-host-tool"))
        .collect();
    assert!(!host_tools.is_empty(), "{batches:?}");
    assert!(
        host_tools.iter().all(|word| *word == INSTALLER_WORD),
        "{host_tools:?}"
    );
    let events = batches[0]["events"].as_array().unwrap().len();
    assert_eq!(
        run.line("relay"),
        format!(
            "relay: delivered {events} event(s) in 1 batch(es) to {hub}/api/v1/telemetry ({events} new at the receiver); hub reporting is permitted for no directory yet, so only a use admitted under reporting consent is cleared to leave"
        )
    );
    assert_eq!(run.line("evidence"), format!("evidence: {hub}/dashboard"));
    assert!(
        run.line("console").starts_with("console: "),
        "{}",
        run.both()
    );
    assert!(
        run.stdout.trim_end().ends_with(
            "Session records stay on this machine. With reporting consent agreed, each use of a \
             source whose licence demands reporting is reported to the telemetry receiver \
             relay.json names, once one is named."
        ),
        "{}",
        run.both()
    );
}

/// Without `--token` the line connects by the hub's device-code flow. The
/// connect step prints the approval page and the code, hands the page with
/// the code filled in to the desktop's opener, and enrols once the hub
/// answers the poll with the enrolment; the rest of the run is the token
/// route's. The device code is in no output and no file.
#[test]
fn without_a_token_the_line_connects_by_a_code_approved_in_the_hub() {
    let world = World::new(true);
    let machine = Machine::new().with_claude_and_codex();
    let _relay = machine.background_relay();
    let bin = machine.bin();
    let hub = world.hub.url();
    let run = world.install(
        &machine,
        &machine.project(),
        &[
            "--dir",
            bin.to_str().unwrap(),
            "--connect",
            &hub,
            "--agree-reporting",
        ],
    );
    assert!(run.status.success(), "{}", run.both());
    let connect: Vec<&str> = run
        .stdout
        .lines()
        .filter(|line| line.starts_with("connect: "))
        .collect();
    assert_eq!(
        connect,
        [
            format!(
                "connect: approve this machine at https://hub.example/connect with the code \
                 {USER_CODE}"
            ),
            format!(
                "connect: enrolled with {hub} in Org A Media as laptop-7; managed, policy \
                 revision 1 in force"
            ),
        ],
        "{}",
        run.both()
    );
    assert_eq!(
        machine.opened(),
        [format!("https://hub.example/connect?code={USER_CODE}")]
    );
    let routes = world.hub_seen.lock().unwrap().routes.clone();
    assert_eq!(
        routes[..2],
        [
            "POST /connect/device",
            "POST /api/v1/enrolment/device/token"
        ],
        "{routes:?}"
    );
    assert!(!routes.contains(&"POST /api/v1/enrolment/exchange".to_owned()));
    assert!(machine.edge().join("enrolment.json").exists());
    assert_eq!(world.batches().len(), 1, "{}", run.both());
    assert!(!run.stdout.contains(DEVICE_CODE) && !run.stderr.contains(DEVICE_CODE));
    for (path, bytes) in machine.files() {
        assert!(
            !String::from_utf8_lossy(&bytes).contains(DEVICE_CODE),
            "{}",
            path.display()
        );
    }
}

/// An opener that stays in the foreground for as long as the browser it
/// started, as `xdg-open` may, does not hold the first run's connect step:
/// the hub is polled and the approval stored while the opener runs, and the
/// run goes on to its end.
#[test]
fn an_opener_that_stays_open_does_not_hold_the_first_run_s_polls() {
    let world = World::new(true);
    let machine = Machine::new().with_claude_and_codex().with_held_opener();
    let _relay = machine.background_relay();
    let bin = machine.bin();
    let hub = world.hub.url();
    let run = world.install(
        &machine,
        &machine.project(),
        &[
            "--dir",
            bin.to_str().unwrap(),
            "--connect",
            &hub,
            "--agree-reporting",
        ],
    );
    assert!(run.status.success(), "{}", run.both());
    let connect: Vec<&str> = run
        .stdout
        .lines()
        .filter(|line| line.starts_with("connect: "))
        .collect();
    assert_eq!(
        connect,
        [
            format!(
                "connect: approve this machine at https://hub.example/connect with the code \
                 {USER_CODE}"
            ),
            format!(
                "connect: enrolled with {hub} in Org A Media as laptop-7; managed, policy \
                 revision 1 in force"
            ),
        ],
        "{}",
        run.both()
    );
    assert!(machine.opener_held(), "the run waited for the opener");
    assert!(machine.edge().join("enrolment.json").exists());
    assert_eq!(world.batches().len(), 1, "{}", run.both());
    machine.release_opener();
}

/// With the opener still running, a code nobody approves expires on the
/// hub's lifetime, counted from the hub's answer: the run stops at connect
/// with the expiry and the retry, and nothing is enrolled.
#[test]
fn an_opener_that_stays_open_does_not_hold_the_first_run_s_expiry() {
    let world = World::new(true);
    world.hub_seen.lock().unwrap().unapproved_lifetime = Some(2);
    let machine = Machine::new().with_claude_and_codex().with_held_opener();
    let bin = machine.bin();
    let hub = world.hub.url();
    let started = Instant::now();
    let run = world.install(
        &machine,
        &machine.project(),
        &["--dir", bin.to_str().unwrap(), "--connect", &hub],
    );
    let elapsed = started.elapsed();
    assert_eq!(run.status.code(), Some(3), "{}", run.both());
    assert!(
        run.stderr.contains(&format!(
            "commonmeasure: connect: the code {USER_CODE} expired before it was approved. \
             Nothing was written; run the line again for a new code. Nothing after it ran"
        )),
        "{}",
        run.both()
    );
    assert!(machine.opener_held(), "the run waited for the opener");
    assert!(!machine.edge().join("enrolment.json").exists());
    let routes = world.hub_seen.lock().unwrap().routes.clone();
    assert_eq!(
        routes,
        [
            "POST /connect/device",
            "POST /api/v1/enrolment/device/token"
        ]
    );
    assert!(elapsed < Duration::from_secs(30), "{elapsed:?}");
    machine.release_opener();
}

/// Each argument error of `--connect` and `--token` exits 2 with the
/// reason, before anything is downloaded: the release records no request,
/// and nothing is placed or written.
#[test]
fn each_argument_error_exits_2_before_any_download() {
    let world = World::new(true);
    let hub = world.hub.url();
    let cases: Vec<(Vec<&str>, &str)> = vec![
        (vec!["--token", TOKEN], "use --token without --connect"),
        (
            vec!["--connect", &hub, "--token", TOKEN, "--update"],
            "combine --connect with --update",
        ),
        (
            vec!["--connect", "http://hub.example", "--token", TOKEN],
            "the hub address must be https, or http to 127.0.0.1, localhost or [::1]",
        ),
        (
            vec!["--connect", "https://ak@hub.example", "--token", TOKEN],
            "a hub address carries no credentials, query, fragment or space",
        ),
        (vec!["--token"], "--token needs a value"),
        (vec!["--connect"], "--connect needs a value"),
    ];
    let mut cases = cases;
    for update in [false, true] {
        for empty in ["", " \t\n "] {
            for mut args in [
                vec!["--connect", empty],
                vec!["--token", empty],
                vec!["--connect", empty, "--token", empty],
                vec!["--connect", &hub, "--token", empty],
                vec!["--connect", empty, "--token", TOKEN],
            ] {
                if update {
                    args.push("--update");
                }
                cases.push((args, "commonmeasure installer: cannot"));
            }
        }
    }
    for (args, reason) in cases {
        let machine = Machine::new();
        let bin = machine.bin();
        let mut all = vec!["--dir", bin.to_str().unwrap()];
        all.extend_from_slice(&args);
        let run = world.install(&machine, &machine.project(), &all);
        assert_eq!(run.status.code(), Some(2), "{args:?}: {}", run.both());
        assert!(run.stderr.contains(reason), "{args:?}: {}", run.both());
        assert!(
            world.release_requests.lock().unwrap().is_empty(),
            "{args:?}: nothing is downloaded"
        );
        assert!(!machine.bin().exists(), "{args:?}");
        assert!(!machine.edge().exists(), "{args:?}");
        assert!(!run.stdout.contains(TOKEN) && !run.stderr.contains(TOKEN));
    }
    assert!(world.hub_seen.lock().unwrap().routes.is_empty());
}

/// A token the hub does not know: the run stops at `connect` with exit 3
/// and the remedy, the binary stays in place, and no later step runs. The
/// hub's refusal quotes the token, and the line does not.
#[test]
fn a_failing_connect_stops_the_run_with_exit_3_and_the_binary_in_place() {
    let world = World::new(true);
    let machine = Machine::new().with_claude_and_codex();
    let padded = format!("  {UNKNOWN_TOKEN}\t ");
    let run = world.first_run(&machine, &machine.project(), &padded, &[]);
    assert_eq!(run.status.code(), Some(3), "{}", run.both());
    assert!(machine.bin().join("commonmeasure").exists());
    // The failed step's line is the binary's error, on stderr; no step
    // after it printed anything.
    assert_steps_in_order(&run, &["hosts"]);
    let connect: Vec<&str> = run
        .stderr
        .lines()
        .filter(|line| line.starts_with("commonmeasure: connect: "))
        .collect();
    assert_eq!(connect.len(), 1, "{}", run.both());
    let connect = connect[0];
    assert!(
        connect.starts_with("commonmeasure: connect: ")
            && connect.contains("<token>")
            && connect.ends_with(
                "Nothing after it ran; mint a new token on the hub and run the line again"
            ),
        "{}",
        run.both()
    );
    assert!(
        run.stderr
            .contains("is installed; the first run stopped at the step named above"),
        "{}",
        run.both()
    );
    assert!(!run.stdout.contains("Session records stay on this machine"));
    assert!(!run.stdout.contains(UNKNOWN_TOKEN) && !run.stderr.contains(UNKNOWN_TOKEN));
    for (path, bytes) in machine.files() {
        assert!(
            !String::from_utf8_lossy(&bytes).contains(UNKNOWN_TOKEN),
            "{}",
            path.display()
        );
    }
    assert!(!machine.edge().join("enrolment.json").exists());
    assert!(session_records(&machine).is_empty());
    assert_eq!(world.page_hits.load(Ordering::SeqCst), 0);
    let routes = world.hub_seen.lock().unwrap().routes.clone();
    assert_eq!(routes, ["POST /api/v1/enrolment/exchange"]);
}

/// Without reporting consent (no terminal to ask on, no flag): the fetch
/// line names `commonmeasure consent agree`, the page is refused before it
/// is requested, and every later step runs; exit 0.
#[test]
fn without_consent_the_fetch_names_the_consent_command_and_the_rest_completes() {
    let world = World::new(true);
    let machine = Machine::new().with_claude_and_codex();
    let _relay = machine.background_relay();
    let run = world.first_run(&machine, &machine.project(), TOKEN, &[]);
    assert!(run.status.success(), "{}", run.both());
    assert_steps_in_order(&run, &STEPS);
    let fetch = run.line("fetch");
    assert!(
        fetch.starts_with(&format!("fetch: {} refused: ", world.page()))
            && fetch.contains("`commonmeasure consent agree`")
            && fetch.contains(
                "reporting consent is not given: sources whose licence demands reporting are \
                 refused until you run `commonmeasure consent agree`"
            ),
        "{}",
        run.both()
    );
    assert_eq!(world.page_hits.load(Ordering::SeqCst), 0);
    let crossings = crossings(&machine);
    assert_eq!(crossings.len(), 1);
    assert_eq!(crossings[0]["event"], "crossing_refused");
    assert_recorded_under_the_installers_word(&machine);
    assert!(run.line("relay").starts_with("relay: "), "{}", run.both());
    assert!(
        run.stdout.contains(
            "Sources whose licence demands usage reporting are refused until you agree: \
             commonmeasure consent agree"
        ),
        "the installer's consent line: {}",
        run.both()
    );
}

/// A hub whose organisation has published no revision: the connect line
/// says the run waits for one and names the hub's page, and the run goes
/// on; exit 0.
#[test]
fn a_hub_with_no_revision_is_reported_as_waiting_and_the_run_goes_on() {
    let world = World::new(false);
    let machine = Machine::new().with_claude_and_codex();
    let _relay = machine.background_relay();
    let run = world.first_run(&machine, &machine.project(), TOKEN, &["--agree-reporting"]);
    assert!(run.status.success(), "{}", run.both());
    assert_steps_in_order(&run, &STEPS);
    assert_eq!(
        run.line("connect"),
        format!(
            "connect: enrolled with {} in Org A Media as laptop-7; waiting for revision: the \
             organisation has published no policy revision, so the local policy stays in force; \
             publish one on the hub's Policy page",
            world.hub.url()
        )
    );
}

/// A directory enrolled for hub reporting before the run keeps its
/// enrolment, and the line says it awaits an owner's approval on the hub.
#[test]
fn a_directory_enrolled_before_the_run_is_reported_awaiting_approval() {
    let world = World::new(true);
    let machine = Machine::new().with_claude_and_codex();
    let _relay = machine.background_relay();
    let mut enrol = Command::new(env!("CARGO_BIN_EXE_commonmeasure"));
    machine.environment(&mut enrol);
    enrol
        .args([
            "enrol",
            "--name",
            "site-2",
            "--reporting",
            "hub",
            "--include-history",
        ])
        .current_dir(machine.project())
        .stdin(Stdio::null());
    let enrolled = Run::of(&mut enrol, None);
    assert!(enrolled.status.success(), "{}", enrolled.both());

    let run = world.first_run(&machine, &machine.project(), TOKEN, &["--agree-reporting"]);
    assert!(run.status.success(), "{}", run.both());
    assert_steps_in_order(&run, &STEPS);
    assert_eq!(
        run.line("enrol"),
        format!(
            "enrol: {} is enrolled as site-2 for hub reporting; awaiting approval: an owner \
             approves it on the hub's Project reporting page",
            machine.project().canonicalize().unwrap().display()
        )
    );
    assert!(
        run.line("relay").ends_with(
            "; hub reporting is permitted for no directory yet, so only a use admitted under \
             reporting consent is cleared to leave"
        ),
        "{}",
        run.both()
    );
}

/// The page refuses: here because the platform has no LaunchAgent, so the
/// first run installs no background relay, nothing else holds the home, and
/// the first run's session, under the installer's word, has no session end,
/// so the licence's reporting demand cannot be met, as
/// `https://commonmeasure.ai/` refuses a fresh machine. The service line
/// names the command to run under the platform's service manager. One line
/// in the edge's words, the page never requested and nothing retried, the
/// run goes on; exit 0. On macOS the first run installs the service; see
/// [`the_relay_installed_first_carries_the_first_run_s_fetch`].
#[cfg(not(target_os = "macos"))]
#[test]
fn a_refused_page_is_one_line_with_no_retry_and_the_run_goes_on() {
    let world = World::new(true);
    let machine = Machine::new().with_claude_and_codex();
    let run = world.first_run(&machine, &machine.project(), TOKEN, &["--agree-reporting"]);
    assert!(run.status.success(), "{}", run.both());
    assert_steps_in_order(&run, &STEPS);
    assert_eq!(
        run.line("service"),
        "service: no background relay holds this edge home; run one under your service \
         manager: commonmeasure relay --every 300"
    );
    let fetch = run.line("fetch");
    assert!(
        fetch.starts_with(&format!("fetch: {} refused: ", world.page()))
            && fetch.contains(&format!(
                "no automatic delivery; this host ({INSTALLER_WORD}) sends no session-end event"
            ))
            && !fetch.contains("consent agree"),
        "{}",
        run.both()
    );
    let crossings = crossings(&machine);
    assert_eq!(crossings.len(), 1, "no retry: {crossings:?}");
    assert_eq!(crossings[0]["event"], "crossing_refused");
    assert_eq!(world.page_hits.load(Ordering::SeqCst), 0);
    assert_recorded_under_the_installers_word(&machine);
}

/// EDG-198. A consenting machine whose platform has LaunchAgents and no
/// relay yet: the first run installs the background relay service before
/// the fetch and waits until its loop holds the edge home, so the page's
/// reporting demand is met. One admitted crossing, one page request, and
/// the hub receives the crossing under the installer's word. The loop
/// started at once may deliver part of it before the relay step does, so
/// the delivery is read across every batch.
#[test]
fn the_relay_installed_first_carries_the_first_run_s_fetch() {
    let world = World::new(true);
    let machine = Machine::new()
        .with_claude_and_codex()
        .with_launch_agents(LAUNCHCTL);
    let run = world.first_run(&machine, &machine.project(), TOKEN, &["--agree-reporting"]);
    assert!(run.status.success(), "{}", run.both());
    assert_steps_in_order(&run, &STEPS);
    assert_eq!(
        run.line("service"),
        "service: background relay service installed; it relays this edge home every 300 s"
    );
    let plist = std::fs::read_to_string(machine.relay_plist()).unwrap();
    assert!(
        plist.contains(&format!(
            "<string>{}</string>\n\t\t<string>relay</string>\n\t\t<string>--every</string>\n\t\t<string>300</string>",
            machine.bin().join("commonmeasure").display()
        )),
        "{plist}"
    );
    // The loop the plist names still holds the home: the ruling read it.
    assert!(
        commonmeasure_harness::delivery::relay_loop_running(&machine.edge()),
        "{}",
        std::fs::read_to_string(machine.launchd().join("relay.log")).unwrap_or_default()
    );

    let crossings = crossings(&machine);
    assert_eq!(crossings.len(), 1, "{crossings:?}");
    let crossing = &crossings[0];
    assert_eq!(crossing["event"], "crossing_mediated", "{crossing}");
    let reporting = &crossing["payload"]["declarations"]["reporting"];
    assert_eq!(reporting["met"], true, "{reporting}");
    assert_eq!(reporting["route"], "hub", "{reporting}");
    assert_eq!(world.page_hits.load(Ordering::SeqCst), 1);
    assert_recorded_under_the_installers_word(&machine);
    assert!(
        run.line("fetch").starts_with(&format!(
            "fetch: {} delivered, mediated; licence ",
            world.page()
        )),
        "{}",
        run.both()
    );

    let batches = world.batches();
    let events: Vec<&Value> = batches
        .iter()
        .flat_map(|batch| batch["events"].as_array().unwrap())
        .collect();
    let host_tools: Vec<&Value> = events
        .iter()
        .filter_map(|event| event["data"].get("contextops-host-tool"))
        .collect();
    assert!(!host_tools.is_empty(), "{batches:?}");
    assert!(
        host_tools.iter().all(|word| *word == INSTALLER_WORD),
        "{host_tools:?}"
    );
    let source = world.page();
    assert!(
        events
            .iter()
            .any(|event| event.to_string().contains(&source)),
        "the crossing's page is among the events delivered: {batches:?}"
    );
    assert!(
        run.line("relay").starts_with("relay: delivered "),
        "{}",
        run.both()
    );
}

/// EDG-198. Where the relay service cannot be installed, here because
/// launchd refuses to load it, the service line names the failure and the
/// command that retries it, and the fetch is refused as on a machine with
/// no relay: one refused crossing, no page request, no retry, and the run
/// goes on; exit 0.
#[test]
fn a_relay_service_that_cannot_be_installed_leaves_the_fetch_refused() {
    let world = World::new(true);
    let machine = Machine::new()
        .with_claude_and_codex()
        .with_launch_agents(LAUNCHCTL_REFUSING);
    let run = world.first_run(&machine, &machine.project(), TOKEN, &["--agree-reporting"]);
    assert!(run.status.success(), "{}", run.both());
    assert_steps_in_order(&run, &STEPS);
    let service = run.line("service");
    assert!(
        service.starts_with("service: installing the background relay service failed: ")
            && service.contains("Bootstrap failed: 5: Input/output error")
            && service.ends_with("Retry with `commonmeasure service install relay`"),
        "{}",
        run.both()
    );
    let fetch = run.line("fetch");
    assert!(
        fetch.starts_with(&format!("fetch: {} refused: ", world.page()))
            && fetch.contains(&format!(
                "no automatic delivery; this host ({INSTALLER_WORD}) sends no session-end event"
            )),
        "{}",
        run.both()
    );
    let crossings = crossings(&machine);
    assert_eq!(crossings.len(), 1, "no retry: {crossings:?}");
    assert_eq!(crossings[0]["event"], "crossing_refused");
    assert_eq!(world.page_hits.load(Ordering::SeqCst), 0);
    assert_recorded_under_the_installers_word(&machine);
}

/// Run from the home directory, as a line pasted into a fresh terminal is:
/// nothing is enrolled, the `enrol` command for a project directory is
/// printed, and the rest runs.
#[test]
fn from_the_home_directory_nothing_is_enrolled_and_the_command_is_printed() {
    let world = World::new(true);
    let machine = Machine::new().with_claude_and_codex();
    let _relay = machine.background_relay();
    let run = world.first_run(&machine, &machine.home(), TOKEN, &["--agree-reporting"]);
    assert!(run.status.success(), "{}", run.both());
    assert_steps_in_order(&run, &STEPS);
    assert_eq!(
        run.line("enrol"),
        format!(
            "enrol: nothing enrolled: {} is your home directory, and hub reporting covers every \
             directory under it; in a project directory run `commonmeasure enrol --name \
             <project> --reporting hub --include-history`",
            machine.home().canonicalize().unwrap().display()
        )
    );
    assert!(!machine.edge().join("directories.json").exists());
}

/// No host on the machine: the hosts line says none is registered and names
/// each as not found, nothing is written for any, and the run goes on.
#[test]
fn with_no_host_present_nothing_is_registered_and_the_run_goes_on() {
    let world = World::new(true);
    let machine = Machine::new();
    let _relay = machine.background_relay();
    let run = world.first_run(&machine, &machine.project(), TOKEN, &["--agree-reporting"]);
    assert!(run.status.success(), "{}", run.both());
    assert_steps_in_order(&run, &STEPS);
    assert_eq!(
        run.line("hosts"),
        "hosts: none registered; not found: claude-code, codex, pi, claude-desktop, cursor, copilot-cli, vscode, chrome"
    );
    assert!(!machine.claude().exists() && !machine.codex().exists());
    assert!(!machine.home().join(".config").exists());
}

/// What the first run registers is, byte for byte, what `install <host>`
/// writes in the same home, and `doctor` reads the two the same.
#[test]
fn registrations_are_byte_for_byte_what_install_writes_and_doctor_sees_no_difference() {
    let world = World::new(true);
    let machine = Machine::new().with_claude_and_codex();
    let _relay = machine.background_relay();
    // Codex's registration also installs the enrol skill under
    // `$HOME/.agents`.
    let skills = machine.home().join(".agents");
    std::fs::create_dir_all(&skills).unwrap();
    let hosts = |machine: &Machine| {
        let mut files = files_under(&machine.claude());
        files.extend(files_under(&machine.codex()));
        files.extend(files_under(&skills));
        files
    };
    let before = hosts(&machine);
    let run = world.first_run(&machine, &machine.project(), TOKEN, &["--agree-reporting"]);
    assert!(run.status.success(), "{}", run.both());
    let doctor = |machine: &Machine| {
        ["claude", "codex"].map(|host| {
            let run = machine.commonmeasure(&machine.project(), &["doctor", host]);
            // The host's own section; the relay's lines above it carry the
            // time since the last delivery.
            let hosts = run
                .stdout
                .split_once("\nHosts\n")
                .map(|(_, hosts)| hosts.to_owned())
                .unwrap_or_else(|| panic!("{host}: {}", run.both()));
            assert!(
                hosts.contains("  registered") && hosts.contains("nothing needs attention"),
                "{host}: {}",
                run.both()
            );
            hosts
        })
    };
    let first_run = hosts(&machine);
    let first_doctor = doctor(&machine);
    assert_ne!(first_run, before, "the first run registered something");

    assert!(
        first_run.keys().any(|path| path.starts_with(&skills)),
        "the enrol skill is among the files compared"
    );
    std::fs::remove_dir_all(&skills).unwrap();
    for dir in [machine.claude(), machine.codex()] {
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
    }
    for (path, bytes) in &before {
        std::fs::write(path, bytes).unwrap();
    }
    for host in ["claude", "codex"] {
        let install = machine.commonmeasure(&machine.project(), &["install", host]);
        assert!(install.status.success(), "{host}: {}", install.both());
    }
    assert_eq!(
        hosts(&machine),
        first_run,
        "install claude and install codex write what the first run wrote"
    );
    assert_eq!(doctor(&machine), first_doctor);
}

/// Echoing hub refusals must not leave the ephemeral enrolment token in
/// terminal output, revocation, proof state, policy state or the relay spool.
/// These are fault-injection fixtures, not live hub acceptance evidence.
#[test]
fn echoing_hub_failures_leave_no_token_in_output_or_files() {
    for failure in [
        EchoFailure::Status,
        EchoFailure::Proof,
        EchoFailure::Signer,
        EchoFailure::Policy,
        EchoFailure::Telemetry,
    ] {
        let mut world = World::new(true);
        let (hub, seen) = hub_with_failure(true, Some(failure));
        world.hub = hub;
        world.hub_seen = seen;
        let machine = Machine::new().with_claude_and_codex();
        let _relay = machine.background_relay();
        let padded = format!(" \t{TOKEN}  ");
        let run = world.first_run(
            &machine,
            &machine.project(),
            &padded,
            &["--agree-reporting"],
        );
        // A later successful status can withdraw a proof-upload refusal;
        // retain that existing connect behaviour rather than changing it.
        let expected = if failure == EchoFailure::Proof { 0 } else { 3 };
        assert_eq!(
            run.status.code(),
            Some(expected),
            "{failure:?}: {}",
            run.both()
        );
        let (method, route) = failure.route();
        assert!(
            world
                .hub_seen
                .lock()
                .unwrap()
                .routes
                .contains(&format!("{method} {route}")),
            "{failure:?}: the failure route must be exercised"
        );
        assert!(!run.stdout.contains(TOKEN), "{failure:?}: {}", run.both());
        assert!(!run.stderr.contains(TOKEN), "{failure:?}: {}", run.both());
        for (path, bytes) in machine.files() {
            assert!(
                !String::from_utf8_lossy(&bytes).contains(TOKEN),
                "{failure:?}: {}",
                path.display()
            );
        }
        if failure == EchoFailure::Status {
            let record: Value = serde_json::from_slice(
                &std::fs::read(machine.edge().join("enrolment.json")).unwrap(),
            )
            .unwrap();
            assert!(
                record["revocation"].as_str().unwrap().contains("<token>"),
                "{record}"
            );
        }
    }
}

#[test]
fn first_run_help_names_the_relay_the_fetch_and_its_host_word() {
    let machine = Machine::new();
    let mut command = Command::new(env!("CARGO_BIN_EXE_commonmeasure"));
    machine.environment(&mut command);
    command.args(["first-run", "--help"]).stdin(Stdio::null());
    let run = Run::of(&mut command, None);
    assert!(run.status.success(), "{}", run.both());
    assert!(run.stdout.contains("--include-history"), "{}", run.both());
    assert!(run.stdout.contains("unenrolled"), "{}", run.both());
    assert!(run.stdout.contains(INSTALLER_WORD), "{}", run.both());
    assert!(!run.stdout.contains("not made"), "{}", run.both());
    assert!(
        run.stdout.contains("install the background relay"),
        "{}",
        run.both()
    );
}

#[test]
fn an_unreadable_file_fails_the_token_scan() {
    use std::os::unix::fs::PermissionsExt;
    let machine = Machine::new();
    let path = machine.root.path().join("unreadable");
    std::fs::write(&path, TOKEN).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    let result = std::panic::catch_unwind(|| machine.files());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        result.is_err(),
        "an unreadable file must not be reported token-free"
    );
}

#[test]
#[should_panic(expected = "scan")]
fn a_failed_file_scan_is_not_token_free() {
    let machine = Machine::new();
    files_under(&machine.root.path().join("missing"));
}

/// The token reaches the hub and nothing else: it is in no line of output
/// and in no file the run leaves, the session records among them.
#[test]
fn the_token_is_in_no_output_and_no_file() {
    let world = World::new(true);
    let machine = Machine::new().with_claude_and_codex();
    let _relay = machine.background_relay();
    let run = world.first_run(&machine, &machine.project(), TOKEN, &["--agree-reporting"]);
    assert!(run.status.success(), "{}", run.both());
    assert!(!run.stdout.contains(TOKEN), "{}", run.both());
    assert!(!run.stderr.contains(TOKEN), "{}", run.both());
    let files = machine.files();
    assert!(
        files
            .keys()
            .any(|path| path.starts_with(machine.edge().join("sessions"))),
        "the session records are among the files read"
    );
    for (path, bytes) in files {
        assert!(
            !String::from_utf8_lossy(&bytes).contains(TOKEN),
            "{}",
            path.display()
        );
    }
}

/// With no controlling terminal and bytes waiting on stdin, the run asks
/// nothing and reads nothing: no prompt is printed, consent is not recorded
/// from the waiting `y`, and the bytes are still there after it ends.
#[test]
fn without_a_terminal_nothing_is_asked_and_nothing_is_read_from_stdin() {
    let world = World::new(true);
    let machine = Machine::new().with_claude_and_codex();
    let _relay = machine.background_relay();
    let (reader, mut writer) = std::io::pipe().unwrap();
    writer.write_all(b"y\ny\n").unwrap();
    let kept = reader.try_clone().unwrap();
    let hub = world.hub.url();
    let bin = machine.bin();
    let mut command = world.installer(&machine, &machine.project());
    command
        .arg(repo_root().join("install.sh"))
        .args([
            "--dir",
            bin.to_str().unwrap(),
            "--connect",
            &hub,
            "--token",
            TOKEN,
        ])
        .stdin(reader);
    let run = Run::of(&mut command, None);
    assert!(run.status.success(), "{}", run.both());
    assert_steps_in_order(&run, &STEPS);
    assert!(!run.stdout.contains("[y/N]") && !run.stderr.contains("[y/N]"));
    assert!(run.line("fetch").contains("`commonmeasure consent agree`"));
    drop(writer);
    let mut left = Vec::new();
    let mut kept = kept;
    kept.read_to_end(&mut left).unwrap();
    assert_eq!(left, b"y\ny\n", "nothing was read from stdin");
}

/// An operator credentials file the edge cannot use stops the run at the
/// fetch, with exit 3: the line names the fetch and the file, says the
/// enrolment stands, and no fetch or relay is made.
#[test]
fn an_unusable_credentials_file_stops_the_run_at_the_fetch_and_names_it() {
    use std::os::unix::fs::PermissionsExt as _;
    let world = World::new(true);
    let machine = Machine::new().with_claude_and_codex();
    let _relay = machine.background_relay();
    let credentials = machine.edge().join("credentials.env");
    std::fs::write(&credentials, "this is not a variable assignment\n").unwrap();
    std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600)).unwrap();
    let run = world.first_run(&machine, &machine.project(), TOKEN, &["--agree-reporting"]);
    assert_eq!(run.status.code(), Some(3), "{}", run.both());
    assert_steps_in_order(&run, &["hosts", "connect", "enrol"]);
    let fetch: Vec<&str> = run
        .stderr
        .lines()
        .filter(|line| line.starts_with("commonmeasure: fetch: "))
        .collect();
    assert_eq!(fetch.len(), 1, "{}", run.both());
    assert!(
        fetch[0].starts_with("commonmeasure: fetch: not made: the governed fetch cannot run: ")
            && fetch[0].contains(&credentials.display().to_string())
            && fetch[0].ends_with(
                "Fix or remove that file; the enrolment above stands, and the next governed \
                 fetch a host makes reads the file again. Nothing after this step ran"
            )
            && !fetch[0].contains("commonmeasure relay"),
        "{}",
        run.both()
    );
    assert!(machine.edge().join("enrolment.json").exists());
    assert!(session_records(&machine).is_empty());
    assert!(world.batches().is_empty());
}

/// The installer's word is not a registered host's: the mediated server a
/// registration starts refuses it under `--host`, before it records
/// anything, so no host's session can be recorded as installer traffic.
#[test]
fn the_installers_word_is_refused_as_a_registered_host() {
    let machine = Machine::new();
    let mut command = Command::new(env!("CARGO_BIN_EXE_commonmeasure"));
    machine.environment(&mut command);
    command
        .args(["mcp", "--host", INSTALLER_WORD])
        .current_dir(machine.project())
        .stdin(Stdio::null());
    let run = Run::of(&mut command, None);
    assert_eq!(run.status.code(), Some(2), "{}", run.both());
    assert!(
        run.stderr
            .contains(&format!("invalid value '{INSTALLER_WORD}'")),
        "{}",
        run.both()
    );
    assert!(!machine.edge().join("sessions").exists(), "{}", run.both());
}
