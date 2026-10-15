//! A source that asks for an anonymous pass: the stand-in gatekeeper's
//! recorded answers (`tests/recorded/pass-presentation/`) served from a
//! loopback origin to the shipped binary, through the production transport,
//! the fetch path and the edge's own wallet.
//!
//! The origin answers as the gatekeeper did: its recorded challenge to a
//! request with no pass, its recorded release to the recorded pass the
//! first time and its recorded refusal of the same pass after that, and its
//! recorded 404 to every other path. The wallet holds the passes the edge's
//! own client obtained from the stand-in Anchor for that recording. The
//! Anchor is never contacted during a fetch, and is named here at a port
//! nothing listens on.

mod common;
use common::{call, crossings, records, write_policy};

mod webbotauth;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use commonmeasure_http::{Request, Response, Server, ServerHandle, read_request, read_response};
use serde_json::{Value, json};

const RECORDED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/recorded/pass-presentation"
);

/// The Anchor the host's terms name. Nothing listens there: presenting a
/// pass never asks the Anchor for one.
const ANCHOR: &str = "http://127.0.0.1:9";

fn recorded(name: &str) -> Vec<u8> {
    std::fs::read(Path::new(RECORDED).join(name)).expect("the recording")
}

fn answer(n: usize) -> Response {
    read_response(&mut recorded(&format!("{n:02}.response")).as_slice()).expect("an answer")
}

fn asked(n: usize) -> Request {
    read_request(&mut recorded(&format!("{n:02}.request")).as_slice()).expect("a request")
}

/// The recorded gatekeeper, keeping every request as it arrived.
fn gatekeeper() -> (ServerHandle, Arc<Mutex<Vec<Request>>>) {
    let (challenge, release, spent, elsewhere) = (answer(1), answer(2), answer(3), answer(4));
    let pass = asked(2)
        .headers
        .get("Authorization")
        .expect("the recorded pass")
        .to_owned();
    assert_eq!(asked(3).headers.get("Authorization"), Some(pass.as_str()));
    let seen: Arc<Mutex<Vec<Request>>> = Arc::default();
    let redeemed = Arc::new(Mutex::new(false));
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn({
            let seen = Arc::clone(&seen);
            move |request: Request| {
                seen.lock().unwrap().push(request.clone());
                if !request.target.starts_with("/article") {
                    return elsewhere.clone();
                }
                match request.headers.get("Authorization") {
                    None => challenge.clone(),
                    Some(presented) if presented == pass => {
                        let mut redeemed = redeemed.lock().unwrap();
                        if std::mem::replace(&mut *redeemed, true) {
                            spent.clone()
                        } else {
                            release.clone()
                        }
                    }
                    Some(_) => Response::text(500, "not in the recording"),
                }
            }
        })
        .expect("spawn");
    (handle, seen)
}

fn policy(home: &Path, scopes: &str) {
    write_policy(
        home,
        &format!(
            r#"{{"policy_mode":"observe","allow_private_hosts":true,{scopes}
                "terms":[{{"host":"127.0.0.1","reference":"anchor-accepted",
                  "anchor":{{"origin":"{ANCHOR}","credential_env":"STAND_IN_ANCHOR_CREDENTIAL"}}}}]}}"#
        ),
    );
}

/// The wallet the recording's refill left, held for [`ANCHOR`].
fn hold_recorded_wallet(home: &Path) -> std::path::PathBuf {
    let path = commonmeasure_harness::passes::wallet_path(home, ANCHOR);
    let wallet = String::from_utf8(recorded("wallet.json"))
        .unwrap()
        .replace("http://anchor.recorded", ANCHOR);
    write_private(&path, wallet.as_bytes());
    path
}

fn write_private(path: &Path, bytes: &[u8]) {
    let directory = path.parent().unwrap();
    std::fs::create_dir_all(directory).unwrap();
    std::fs::write(path, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

/// An edge enrolled with a hub, so that a request it sends unsigned is
/// one it could have signed.
fn enrolled_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("tempdir");
    let mut directory = webbotauth::Directory::default();
    webbotauth::enrol(
        home.path(),
        "https://hub.example",
        "https://hub.example",
        &mut directory,
    );
    home
}

fn fetch(home: &Path, url: &str) -> Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["mcp", "--host", "claude-code", "--session", "test-session"])
        .env("COMMONMEASURE_HOME", home)
        .env_remove("AGENT_SESSION_ID")
        .env_remove("STAND_IN_ANCHOR_CREDENTIAL")
        .current_dir(home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the binary should start");
    writeln!(
        child.stdin.as_mut().unwrap(),
        "{}",
        call("context_fetch", json!({"url": url}))
    )
    .unwrap();
    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success(), "the server should exit cleanly");
    let line = String::from_utf8_lossy(&output.stdout)
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .expect("one response");
    serde_json::from_str(&line).expect("JSON")
}

fn told(response: &Value) -> String {
    commonmeasure_harness::provenance::payload_text(&response["result"])
        .expect("an error result carries text")
        .to_owned()
}

/// Under the profile no request carries a signature, a bot page, a contact
/// or a correlation id: the page request, the request carrying the pass and
/// every probe.
fn assert_anonymous(seen: &[Request]) {
    assert!(!seen.is_empty());
    let product = format!("CommonMeasureBot/{}", env!("CARGO_PKG_VERSION"));
    for request in seen {
        for name in [
            "Signature",
            "Signature-Input",
            "Signature-Agent",
            "Content-Telemetry-ID",
        ] {
            assert!(
                request.headers.get(name).is_none(),
                "{} carries {name}",
                request.target
            );
        }
        assert_eq!(
            request.headers.get("User-Agent"),
            Some(product.as_str()),
            "{}",
            request.target
        );
    }
}

fn article(site: &ServerHandle) -> String {
    format!("{}/article?title=Harbour+Bridge+Repairs", site.url())
}

/// Challenge, then the release: the edge answers the source's challenge
/// once with a held pass, unsigned though it is enrolled, and the crossing
/// says which Anchor's pass it presented and nothing that identifies it.
#[test]
fn a_challenge_is_answered_once_with_a_held_pass_and_the_release_is_delivered() {
    let (site, seen) = gatekeeper();
    let home = enrolled_home();
    policy(home.path(), "");
    let wallet = hold_recorded_wallet(home.path());
    let pass = asked(2).headers.get("Authorization").unwrap().to_owned();

    let response = fetch(home.path(), &article(&site));
    assert_eq!(response["result"]["isError"], false, "{response}");

    let seen = seen.lock().unwrap();
    assert_anonymous(&seen);
    let pages: Vec<&Request> = seen
        .iter()
        .filter(|request| request.target.starts_with("/article"))
        .collect();
    assert_eq!(pages.len(), 2, "the page, then once with the pass");
    assert_eq!(pages[0].headers.get("Authorization"), None);
    assert_eq!(pages[1].headers.get("Authorization"), Some(pass.as_str()));
    assert!(
        seen.iter().any(|request| request.target == "/robots.txt"),
        "the probe was sent, unsigned"
    );

    let crossing = &crossings(home.path())[0];
    assert_eq!(crossing["event"], "crossing_mediated");
    let payload = &crossing["payload"];
    assert_eq!(payload["http_status"], 200);
    assert_eq!(payload["release"]["verification"], "verified");
    assert_eq!(payload["anonymous"], json!({"anchor": ANCHOR}));
    assert!(payload["content_telemetry_id"].is_null());
    let identity = &payload["identity"];
    let held: Value = serde_json::from_slice(&recorded("wallet.json")).unwrap();
    let key_id = held["key_id"].as_str().unwrap();
    let truncated = u8::from_str_radix(&key_id[62..], 16).unwrap();
    assert_eq!(
        identity,
        &json!({
            "user_agent": format!("CommonMeasureBot/{}", env!("CARGO_PKG_VERSION")),
            "pass": {"anchor": ANCHOR, "truncated_token_key_id": truncated},
        }),
        "no key_id, signature_agent or reason beside the pass"
    );
    let log = records(home.path());
    let text = serde_json::to_string(&log).unwrap();
    let token = pass.strip_prefix("PrivateToken token=").unwrap();
    assert!(!text.contains(token), "the record carries no pass");
    assert!(!text.contains(key_id), "nor the whole key id");

    let left: Value = serde_json::from_slice(&std::fs::read(&wallet).unwrap()).unwrap();
    assert_eq!(left["passes"].as_array().unwrap().len(), 1, "one was spent");
}

/// Double spend: a pass the source has already redeemed (here, the wallet
/// file restored after the first fetch, as a copied wallet would be) is
/// presented once, refused by the source with a new challenge, and the
/// refusal is recorded; the edge does not try again.
#[test]
fn a_pass_the_source_already_redeemed_is_refused_and_recorded() {
    let (site, seen) = gatekeeper();
    let home = enrolled_home();
    policy(home.path(), "");
    let wallet = hold_recorded_wallet(home.path());
    let copy = std::fs::read(&wallet).unwrap();
    let first = fetch(home.path(), &article(&site));
    assert_eq!(first["result"]["isError"], false, "{first}");
    write_private(&wallet, &copy);

    let second = fetch(home.path(), &article(&site));
    assert_eq!(second["result"]["isError"], true, "{second}");
    let message = told(&second);
    assert!(message.contains("did not accept the pass"), "{message}");

    let seen = seen.lock().unwrap();
    assert_anonymous(&seen);
    let pages = seen
        .iter()
        .filter(|request| request.target.starts_with("/article"))
        .count();
    assert_eq!(pages, 4, "each fetch asked twice, and no more");

    let crossing = &crossings(home.path())[1];
    assert_eq!(crossing["event"], "crossing_mediated");
    let payload = &crossing["payload"];
    assert_eq!(payload["http_status"], 401);
    assert!(payload["content_hash"].is_null(), "nothing was retrieved");
    let challenge = payload["challenge"].as_str().unwrap();
    assert!(challenge.contains("did not accept the pass"), "{challenge}");
    assert!(payload["identity"]["pass"].is_object());
    assert_eq!(payload["anonymous"], json!({"anchor": ANCHOR}));
}

/// An empty wallet: the challenge is not answered, the fetch is refused
/// with the gap named, and nothing is sent without the pass, signed or
/// unsigned.
#[test]
fn an_empty_wallet_refuses_the_fetch_and_names_the_refill() {
    let (site, seen) = gatekeeper();
    let home = enrolled_home();
    policy(home.path(), "");

    let response = fetch(home.path(), &article(&site));
    assert_eq!(response["result"]["isError"], true, "{response}");
    let message = told(&response);
    assert!(message.contains("asked for an anonymous pass"), "{message}");
    assert!(message.contains("commonmeasure passes refill"), "{message}");

    let seen = seen.lock().unwrap();
    assert_anonymous(&seen);
    let pages = seen
        .iter()
        .filter(|request| request.target.starts_with("/article"))
        .count();
    assert_eq!(pages, 1, "the challenged request alone");

    let crossing = &crossings(home.path())[0];
    assert_eq!(crossing["event"], "crossing_refused");
    let payload = &crossing["payload"];
    assert_eq!(payload["http_status"], 401);
    let refusal = payload["refusal"].as_str().unwrap();
    assert!(refusal.contains("holds no pass for 127.0.0.1"), "{refusal}");
    let challenge = payload["challenge"].as_str().unwrap();
    assert!(challenge.contains("PrivateToken challenge"), "{challenge}");
    let identity = &payload["identity"];
    assert!(identity["key_id"].is_null() && identity["pass"].is_null());
    let unsigned = identity["unsigned"].as_str().unwrap();
    assert!(unsigned.contains("anonymous access profile"), "{unsigned}");
    assert_eq!(payload["anonymous"], json!({"anchor": ANCHOR}));
}

/// After the Anchor rotates its key, the source's challenge names the new
/// one: the held passes, under the old key, are not spent on it, and the
/// refusal says so.
#[test]
fn passes_under_a_key_the_challenge_does_not_name_are_not_presented() {
    let mut rotated = answer(1);
    let header = rotated.headers.get("WWW-Authenticate").unwrap().to_owned();
    let (head, key) = header.split_once("token-key=").unwrap();
    // Another key of the same shape: the recorded one with one modulus
    // byte changed, which is still an RSA key and has another id.
    let mut bytes =
        base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE, key.trim()).unwrap();
    let last = bytes.len() - 10;
    bytes[last] ^= 2;
    let other = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE, &bytes);
    rotated
        .headers
        .set("WWW-Authenticate", &format!("{head}token-key={other}"));
    let seen: Arc<Mutex<Vec<Request>>> = Arc::default();
    let site = Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn({
            let seen = Arc::clone(&seen);
            let elsewhere = answer(4);
            move |request: Request| {
                seen.lock().unwrap().push(request.clone());
                if request.target.starts_with("/article") {
                    rotated.clone()
                } else {
                    elsewhere.clone()
                }
            }
        })
        .unwrap();
    let home = enrolled_home();
    policy(home.path(), "");
    let wallet = hold_recorded_wallet(home.path());

    let response = fetch(home.path(), &article(&site));
    assert_eq!(response["result"]["isError"], true, "{response}");
    let crossing = &crossings(home.path())[0];
    assert_eq!(crossing["event"], "crossing_refused");
    let refusal = crossing["payload"]["refusal"].as_str().unwrap();
    assert!(
        refusal.contains("other than the one the challenge names"),
        "{refusal}"
    );
    assert_anonymous(&seen.lock().unwrap());
    let left: Value = serde_json::from_slice(&std::fs::read(&wallet).unwrap()).unwrap();
    assert_eq!(left["passes"].as_array().unwrap().len(), 2, "none spent");
}

/// A redirect from the challenged host to a host the terms do not put under
/// the same Anchor is not followed, with the pass or without it.
#[test]
fn a_redirect_off_the_profile_is_not_followed() {
    let elsewhere = common::origin("a page on another host");
    let target = format!("{}/landing", elsewhere.url()).replace("127.0.0.1", "localhost");
    let seen: Arc<Mutex<Vec<Request>>> = Arc::default();
    let site = Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn({
            let seen = Arc::clone(&seen);
            let not_here = answer(4);
            move |request: Request| {
                seen.lock().unwrap().push(request.clone());
                if request.target.starts_with("/article") {
                    let mut moved = Response::text(302, "");
                    moved.headers.set("Location", &target);
                    moved
                } else {
                    not_here.clone()
                }
            }
        })
        .unwrap();
    let home = enrolled_home();
    policy(home.path(), "");
    hold_recorded_wallet(home.path());

    let response = fetch(home.path(), &article(&site));
    assert_eq!(response["result"]["isError"], true, "{response}");
    let message = told(&response);
    assert!(
        message.contains("not under the anonymous access profile"),
        "{message}"
    );
    let crossing = &crossings(home.path())[0];
    assert_eq!(crossing["event"], "crossing_refused");
    assert_eq!(crossing["payload"]["anonymous"], json!({"anchor": ANCHOR}));
    assert_anonymous(&seen.lock().unwrap());
}

/// A signed fetch of a host not under the profile whose `robots.txt`
/// redirects to a host under it: the probe's redirect is declined, so the
/// host under the profile is never sent a request that follows a signed
/// one.
#[test]
fn a_signed_probe_is_not_carried_to_a_host_under_the_profile() {
    let (site, seen) = gatekeeper();
    let robots = format!("{}/robots.txt", site.url());
    let plain = Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(move |request: Request| {
            if request.target == "/robots.txt" {
                let mut moved = Response::text(302, "");
                moved.headers.set("Location", &robots);
                moved
            } else {
                Response::text(200, "a page on a host not under the profile")
            }
        })
        .unwrap();
    let home = enrolled_home();
    policy(home.path(), "");

    let page = format!("{}/page", plain.url()).replace("127.0.0.1", "localhost");
    fetch(home.path(), &page);
    assert!(
        seen.lock().unwrap().is_empty(),
        "the host under the profile was asked nothing"
    );
    let crossing = serde_json::to_string(&crossings(home.path())[0]).unwrap();
    assert!(crossing.contains("this probe was signed"), "{crossing}");
}
