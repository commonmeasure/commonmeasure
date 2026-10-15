//! `commonmeasure passes refill` through the shipped binary: the term it
//! needs, the credential it reads and keeps from any origin but the
//! Anchor's, and an Anchor's refusal recorded as the
//! Anchor stated it. The Anchor is the recorded stand-in
//! (`crates/commonmeasure-harness/tests/recorded/anchor-issuance/`) served
//! from loopback.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use commonmeasure_http::{Request, Response, Server, ServerHandle, read_response};
use serde_json::{Value, json};

const HOST: &str = "source.example";
const VARIABLE: &str = "COMMONMEASURE_TEST_ANCHOR_CREDENTIAL";

fn recorded(name: &str) -> Response {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../commonmeasure-harness/tests/recorded/anchor-issuance")
        .join(name);
    let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    read_response(&mut bytes.as_slice()).expect("a recorded answer")
}

/// The recorded stand-in, answering its directory read and refusing every
/// token request with its recorded ration refusal; every request kept.
fn refusing_anchor() -> (ServerHandle, Arc<Mutex<Vec<Request>>>) {
    let directory = recorded("01.response");
    let refusal = recorded("05.response");
    let received: Arc<Mutex<Vec<Request>>> = Arc::default();
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn({
            let received = received.clone();
            move |request: Request| {
                let answer = if request.method == "GET" {
                    directory.clone()
                } else {
                    refusal.clone()
                };
                received.lock().unwrap().push(request);
                answer
            }
        })
        .expect("spawn");
    (handle, received)
}

fn policy(anchor: Option<&str>) -> String {
    let mut terms = json!({"host": HOST, "reference": "stand-in"});
    if let Some(origin) = anchor {
        terms["anchor"] = json!({"origin": origin, "credential_env": VARIABLE});
    }
    json!({"allow_private_hosts": true, "terms": [terms]}).to_string()
}

fn refill(home: &Path, credential: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_commonmeasure"));
    command
        .args(["passes", "refill", HOST, "--count", "2", "--session", "s"])
        .env("COMMONMEASURE_HOME", home)
        .env_remove(VARIABLE)
        .current_dir(home);
    if let Some(credential) = credential {
        command.env(VARIABLE, credential);
    }
    command.output().expect("run the binary")
}

fn wallets(home: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(home.join("passes"))
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_host_whose_terms_name_no_anchor_gets_no_passes() {
    let home = tempfile::tempdir().unwrap();
    common::write_policy(home.path(), &policy(None));
    let output = refill(home.path(), Some("stand-in-credential"));
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!("names no anchor for {HOST}")),
        "{stderr}"
    );
    assert!(wallets(home.path()).is_empty());
    assert!(!home.path().join("sessions/s.ndjson").exists());
}

#[test]
fn an_unset_credential_variable_asks_nothing_of_the_anchor() {
    let (mut anchor, received) = refusing_anchor();
    let home = tempfile::tempdir().unwrap();
    common::write_policy(home.path(), &policy(Some(&anchor.url())));
    let output = refill(home.path(), None);
    anchor.stop();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!("{VARIABLE} is not set")),
        "{stderr}"
    );
    assert!(received.lock().unwrap().is_empty(), "the anchor was asked");
    assert!(wallets(home.path()).is_empty());
}

/// The recorded ration refusal answers the first token request: nothing is
/// issued, the refill fails, and the session log records the refusal as the
/// Anchor stated it. The binary asks without a signature or a user agent.
#[test]
fn an_anchor_refusal_is_recorded_as_stated() {
    let (mut anchor, received) = refusing_anchor();
    let origin = anchor.url();
    let home = tempfile::tempdir().unwrap();
    common::write_policy(home.path(), &policy(Some(&origin)));
    let output = refill(home.path(), Some("stand-in-credential"));
    anchor.stop();

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("obtained    0 of 2"), "{stdout}");
    assert!(stderr.contains("issued no passes"), "{stderr}");

    let received = received.lock().unwrap();
    assert_eq!(
        received
            .iter()
            .map(|request| request.method.as_str())
            .collect::<Vec<_>>(),
        ["GET", "POST"],
        "the directory, then one token request, stopped by the refusal"
    );
    for request in received.iter() {
        for name in [
            "Signature",
            "Signature-Input",
            "Signature-Agent",
            "User-Agent",
        ] {
            assert!(request.headers.get(name).is_none(), "{name} was sent");
        }
    }

    let records = common::session_records(home.path(), "s");
    let refilled: Vec<&Value> = records
        .iter()
        .filter(|record| record["event"] == "passes_refilled")
        .collect();
    assert_eq!(refilled.len(), 1);
    let payload = &refilled[0]["payload"];
    assert_eq!(payload["anchor"], origin);
    assert_eq!(payload["origin_info"], json!([HOST]));
    assert_eq!(payload["requested"], 2);
    assert_eq!(payload["obtained"], 0);
    assert_eq!(payload["held"], 0);
    assert_eq!(
        payload["stopped"],
        json!({"status": 429, "retry_after": null,
               "stated": {"error": "ration exhausted", "ration_per_window": 3}})
    );
    let log = std::fs::read_to_string(home.path().join("sessions/s.ndjson")).unwrap();
    assert!(!log.contains("stand-in-credential"));

    let wallets = wallets(home.path());
    assert_eq!(wallets.len(), 1);
    let wallet: Value = serde_json::from_slice(&std::fs::read(&wallets[0]).unwrap()).unwrap();
    assert_eq!(wallet["anchor"], origin);
    assert_eq!(wallet["key_id"], payload["key_id"]);
    assert_eq!(wallet["passes"], json!([]));
}

/// An Anchor whose directory sends token requests to another origin gets no
/// token request, and that origin never sees the credential: the refill
/// fails on the directory, before anything is sent. The directory is the
/// recorded one with only its `issuer-request-uri` made absolute on a
/// second loopback origin.
#[test]
fn a_directory_naming_another_origin_gets_no_credential() {
    let elsewhere_received: Arc<Mutex<Vec<Request>>> = Arc::default();
    let mut elsewhere = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn({
            let received = elsewhere_received.clone();
            move |request: Request| {
                received.lock().unwrap().push(request);
                Response::text(500, "not an anchor")
            }
        })
        .expect("spawn");
    let mut directory = recorded("01.response");
    let mut listing: Value = serde_json::from_slice(&directory.body).unwrap();
    listing["issuer-request-uri"] = json!(format!("{}/token-request", elsewhere.url()));
    directory.body = serde_json::to_vec(&listing).unwrap();
    let received: Arc<Mutex<Vec<Request>>> = Arc::default();
    let mut anchor = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn({
            let received = received.clone();
            move |request: Request| {
                received.lock().unwrap().push(request);
                directory.clone()
            }
        })
        .expect("spawn");
    let home = tempfile::tempdir().unwrap();
    common::write_policy(home.path(), &policy(Some(&anchor.url())));
    let output = refill(home.path(), Some("stand-in-credential"));
    anchor.stop();
    elsewhere.stop();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("outside the anchor's origin"), "{stderr}");
    assert!(
        elsewhere_received.lock().unwrap().is_empty(),
        "the other origin was asked"
    );
    let received = received.lock().unwrap();
    assert!(received.iter().all(|request| request.method == "GET"));
    assert!(
        received
            .iter()
            .all(|request| request.headers.get("Authorization").is_none()),
        "the credential went with the directory read"
    );
}
