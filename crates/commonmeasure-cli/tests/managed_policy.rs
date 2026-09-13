//! Signed policy distribution driven through the real binary against a
//! loopback origin in place of the hub's policy endpoint: the same HTTP
//! client, signer, verifier, policy loader, atomic save, session log and
//! status document a managed edge uses. The origin serves envelopes signed
//! with a real Ed25519 key and admits only requests carrying a valid Web Bot
//! Auth signature from a key in its directory, which is both halves of the
//! exchange the hub makes (`docs/contracts/policy-envelope.md`).

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use commonmeasure_harness::fleet::{Convergence, classify};
use commonmeasure_harness::managed::{ENVELOPE, encode_hex};
use commonmeasure_harness::policy::PolicyFile;
use commonmeasure_http::{Response, Server, ServerHandle};
use commonmeasure_types::canonical::{canonical_digest, canonical_json};
use ring::signature::KeyPair;
use serde_json::{Value, json};

mod webbotauth;

/// The hub's signing side: a key pair and the envelopes it signs.
struct Signer {
    pair: ring::signature::Ed25519KeyPair,
    key_id: &'static str,
}

impl Signer {
    fn new(key_id: &'static str) -> Self {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).expect("a key pair");
        Self {
            pair: ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("pkcs8"),
            key_id,
        }
    }

    fn public_key_hex(&self) -> String {
        encode_hex(self.pair.public_key().as_ref())
    }

    fn envelope(&self, revision: u64, policy: Value) -> Vec<u8> {
        let normalised = serde_json::from_value::<PolicyFile>(policy.clone())
            .map(|policy| serde_json::to_value(&policy).expect("serialises"))
            .unwrap_or(policy.clone());
        let payload = json!({
            "organisation": "org-1",
            "edge_key_id": null,
            "revision": revision,
            "issued_at": "2026-09-06T00:00:00Z",
            "expires_at": "2099-01-01T00:00:00Z",
            "policy": policy,
            "digest": canonical_digest(&normalised),
        });
        let signature = self.pair.sign(canonical_json(&payload).as_bytes());
        serde_json::to_vec(&json!({
            "envelope": ENVELOPE,
            "payload": payload,
            "signature": {
                "algorithm": "ed25519",
                "key_id": self.key_id,
                "value": encode_hex(signature.as_ref()),
            },
        }))
        .expect("serialises")
    }
}

/// The policy endpoint: serves whatever envelope is set to an edge whose
/// signature verifies, counts requests, and records whether any request
/// carried a telemetry key or the key id that signed it.
struct Endpoint {
    handle: ServerHandle,
    served: Arc<Mutex<Vec<u8>>>,
    requests: Arc<AtomicUsize>,
    carried_telemetry_key: Arc<AtomicUsize>,
    /// The key id of each request that verified, in order.
    authenticated: Arc<Mutex<Vec<String>>>,
    /// Why a request was refused, for the ones that did not verify.
    refused: Arc<Mutex<Vec<String>>>,
}

fn endpoint(first: Vec<u8>, directory: Arc<Mutex<webbotauth::Directory>>) -> Endpoint {
    let served = Arc::new(Mutex::new(first));
    let requests = Arc::new(AtomicUsize::new(0));
    let carried_telemetry_key = Arc::new(AtomicUsize::new(0));
    let authenticated = Arc::new(Mutex::new(Vec::new()));
    let refused = Arc::new(Mutex::new(Vec::new()));
    let (handler_served, handler_requests, handler_key) = (
        Arc::clone(&served),
        Arc::clone(&requests),
        Arc::clone(&carried_telemetry_key),
    );
    let (handler_authenticated, handler_refused, handler_directory) = (
        Arc::clone(&authenticated),
        Arc::clone(&refused),
        Arc::clone(&directory),
    );
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |request| {
            handler_requests.fetch_add(1, Ordering::SeqCst);
            if request.headers.get("X-API-Key").is_some() {
                handler_key.fetch_add(1, Ordering::SeqCst);
            }
            assert_eq!(request.target, "/api/v1/policy/desired");
            // The authority is read from `Host`, as a real origin reads it:
            // the signature binds the request to the origin it was made for,
            // so one captured from a publisher cannot be replayed here.
            let authority = request.headers.get("Host").unwrap_or_default().to_owned();
            match webbotauth::verify(
                &request,
                &authority,
                &handler_directory.lock().expect("lock"),
            ) {
                Ok(verified) => {
                    handler_authenticated
                        .lock()
                        .expect("lock")
                        .push(verified.key_id);
                    let body = handler_served.lock().expect("lock").clone();
                    let mut response = Response::new(200, body);
                    response.headers.set("Content-Type", "application/json");
                    response
                }
                Err(reason) => {
                    handler_refused.lock().expect("lock").push(reason);
                    Response::json(
                        401,
                        r#"{"detail":"the request is not signed by an enrolled key"}"#,
                    )
                }
            }
        })
        .expect("spawn");
    Endpoint {
        handle,
        served,
        requests,
        carried_telemetry_key,
        authenticated,
        refused,
    }
}

fn run(home: &Path, cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(args)
        .env("COMMONMEASURE_HOME", home)
        .current_dir(cwd)
        .output()
        .expect("the binary runs")
}

fn status(home: &Path, cwd: &Path) -> Value {
    let output = run(home, cwd, &["status", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("the document is JSON")
}

fn write_deployment(home: &Path, signer: &Signer, policy_url: &str) {
    std::fs::write(
        home.join("deployment.json"),
        json!({
            "mode": "managed",
            "signer": {"key_id": signer.key_id, "algorithm": "ed25519",
                       "public_key": signer.public_key_hex()},
            "policy_url": policy_url,
            "organisation": "org-1",
        })
        .to_string(),
    )
    .expect("deployment");
}

fn policy_mode(home: &Path) -> String {
    let file: Value =
        serde_json::from_slice(&std::fs::read(home.join("policy.json")).expect("policy"))
            .expect("json");
    file["policy_mode"].as_str().expect("mode").to_owned()
}

/// A mediated fetch through the real MCP server, with no hub involved:
/// whether the policy in force refused it.
fn mediated_fetch_refused(home: &Path, url: &str) -> bool {
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args([
            "mcp",
            "--host",
            "claude-code",
            "--session",
            "managed-session",
        ])
        .env("COMMONMEASURE_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the server starts");
    writeln!(
        child.stdin.as_mut().expect("stdin"),
        "{}",
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
               "params": {"name": "context_fetch", "arguments": {"url": url}}})
    )
    .expect("write");
    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success());
    let response: Value = serde_json::from_str(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .find(|line| !line.trim().is_empty())
            .expect("one response"),
    )
    .expect("json");
    response["result"]["isError"] == json!(true)
}

const STRICT: &str = r#"{"policy_mode":"strict","allow_private_hosts":true,
    "constraints":[{"kind":"allowed_source_host","host":"www.gov.uk"}]}"#;

/// The rollout from desired to applied, drift visible before convergence,
/// rollback refused, the last-known-good kept across every failure and
/// enforced offline, and a local edge that makes no request.
#[test]
fn a_managed_edge_activates_the_signed_policy_and_keeps_the_last_known_good_across_failure() {
    let signer = Signer::new("hub-policy-1");
    let strict: Value = serde_json::from_str(STRICT).expect("policy");
    let directory = Arc::new(Mutex::new(webbotauth::Directory::default()));
    let hub = endpoint(signer.envelope(1, strict.clone()), Arc::clone(&directory));
    let policy_url = format!("{}/api/v1/policy/desired", hub.handle.url());
    let home = tempfile::tempdir().expect("tempdir");
    let workspace = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        home.path().join("policy.json"),
        r#"{"policy_mode":"observe","allow_private_hosts":true}"#,
    )
    .expect("policy");
    let page = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(|_| Response::text(200, "a page"))
        .expect("spawn");
    let page_url = format!("{}/page", page.url());

    // Local mode first: the default, and it refuses to fetch anything.
    let refused = run(home.path(), workspace.path(), &["policy", "sync"]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("deployment mode is local"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert_eq!(
        hub.requests.load(Ordering::SeqCst),
        0,
        "a local edge makes no request"
    );
    let before = status(home.path(), workspace.path());
    assert_eq!(before["deployment_mode"], "local");
    assert!(before["desired"].is_null());
    assert!(
        !mediated_fetch_refused(home.path(), &page_url),
        "observe carries the fetch"
    );

    // Managed but not enrolled: the endpoint authenticates the edge by the
    // key enrolment mints, so there is nothing to sign with and no request to
    // make. The reason names the missing enrolment rather than the hub.
    write_deployment(home.path(), &signer, &policy_url);
    let unenrolled = run(home.path(), workspace.path(), &["policy", "sync"]);
    assert!(!unenrolled.status.success());
    let text = String::from_utf8_lossy(&unenrolled.stdout);
    assert!(text.contains("outcome       unauthenticated"), "{text}");
    assert!(text.contains("not enrolled"), "{text}");
    assert_eq!(
        hub.requests.load(Ordering::SeqCst),
        0,
        "an edge with no key sends nothing to the endpoint"
    );

    // Enrolled: the edge signs with the key the hub published, and the
    // endpoint admits it on the signature alone.
    let key_id = webbotauth::enrol(
        home.path(),
        &hub.handle.url(),
        "https://hub.example",
        &mut directory.lock().expect("lock"),
    );

    // Managed: the desired revision is visible as not yet applied, then
    // applied.
    let strict_digest = canonical_digest(
        &serde_json::to_value(serde_json::from_value::<PolicyFile>(strict.clone()).unwrap())
            .unwrap(),
    );
    let unsynced = status(home.path(), workspace.path());
    assert_eq!(unsynced["deployment_mode"], "managed");
    assert!(unsynced["applied"]["revision"].is_null());
    assert_eq!(
        classify(&unsynced, 1, &strict_digest).0,
        Convergence::Unknown
    );

    let accepted = run(home.path(), workspace.path(), &["policy", "sync"]);
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    let text = String::from_utf8_lossy(&accepted.stdout);
    assert!(
        text.contains("outcome       accepted (revision 1)"),
        "{text}"
    );
    assert_eq!(
        policy_mode(home.path()),
        "strict",
        "the distributed policy is on disk"
    );
    assert_eq!(hub.carried_telemetry_key.load(Ordering::SeqCst), 0);
    assert_eq!(
        *hub.authenticated.lock().expect("lock"),
        vec![key_id.clone()],
        "the hub authenticated the edge by its enrolled key and nothing else"
    );
    assert!(
        hub.refused.lock().expect("lock").is_empty(),
        "{:?}",
        hub.refused.lock().expect("lock")
    );
    assert!(
        home.path().join("managed/last-known-good.json").exists(),
        "the accepted envelope is kept verbatim"
    );
    let synced = status(home.path(), workspace.path());
    assert_eq!(synced["applied"]["revision"], 1);
    assert_eq!(synced["applied"]["policy_digest"], strict_digest);
    assert_eq!(synced["desired"]["outcome"], "accepted");
    assert_eq!(synced["desired"]["revision"], 1);
    assert_eq!(classify(&synced, 1, &strict_digest).0, Convergence::Current);
    assert!(
        mediated_fetch_refused(home.path(), &page_url),
        "strict now refuses the page"
    );

    // Drift after activation: a local edit shows as divergent, and the
    // next synchronisation reapplies the desired revision.
    std::fs::write(
        home.path().join("policy.json"),
        r#"{"policy_mode":"observe","allow_private_hosts":true}"#,
    )
    .expect("edit");
    let drifted = status(home.path(), workspace.path());
    assert_eq!(drifted["applied"]["revision"], 1);
    assert_eq!(
        classify(&drifted, 1, &strict_digest).0,
        Convergence::Divergent
    );
    let reapplied = run(home.path(), workspace.path(), &["policy", "sync"]);
    assert!(reapplied.status.success());
    assert!(String::from_utf8_lossy(&reapplied.stdout).contains("reapplied"));
    assert_eq!(
        classify(&status(home.path(), workspace.path()), 1, &strict_digest).0,
        Convergence::Current
    );

    // A newer desired revision the edge has not fetched yet: stale.
    let (verdict, _) = classify(&status(home.path(), workspace.path()), 2, "sha256:next");
    assert_eq!(verdict, Convergence::Stale);

    // Rollback: revision 0, validly signed, with a looser policy. Refused;
    // strict stays; the failure is on the status document.
    *hub.served.lock().unwrap() = signer.envelope(
        0,
        json!({"policy_mode": "observe", "allow_private_hosts": true}),
    );
    let rollback = run(home.path(), workspace.path(), &["policy", "sync"]);
    assert!(!rollback.status.success());
    assert!(String::from_utf8_lossy(&rollback.stdout).contains("rollback refused"));
    assert_eq!(policy_mode(home.path()), "strict");
    let after_rollback = status(home.path(), workspace.path());
    assert_eq!(after_rollback["desired"]["outcome"], "rejected");
    assert_eq!(after_rollback["applied"]["revision"], 1);
    assert_eq!(
        classify(&after_rollback, 1, &strict_digest).0,
        Convergence::Current
    );

    // Wrong signer, wrong organisation, expired, an edge-bound envelope on
    // an edge with no key, and a policy the loader refuses: each rejected
    // by name, none written.
    let forger = Signer::new("hub-policy-1");
    *hub.served.lock().unwrap() = forger.envelope(2, json!({"policy_mode": "observe"}));
    let forged = run(home.path(), workspace.path(), &["policy", "sync"]);
    assert!(!forged.status.success());
    assert!(String::from_utf8_lossy(&forged.stdout).contains("bad_signature"));

    let mut envelope: Value = serde_json::from_slice(&signer.envelope(2, strict.clone())).unwrap();
    envelope["payload"]["organisation"] = json!("org-2");
    *hub.served.lock().unwrap() = serde_json::to_vec(&envelope).unwrap();
    let other_org = run(home.path(), workspace.path(), &["policy", "sync"]);
    assert!(!other_org.status.success());
    assert!(
        String::from_utf8_lossy(&other_org.stdout).contains("bad_signature"),
        "a payload edited after signing fails the signature before the organisation is read"
    );

    let mut payload = json!({
        "organisation": "org-1", "edge_key_id": "edge-someone-else", "revision": 2,
        "issued_at": "2026-09-06T00:00:00Z", "expires_at": "2099-01-01T00:00:00Z",
        "policy": strict.clone(), "digest": strict_digest,
    });
    let bound = signed_by(&signer, &payload);
    *hub.served.lock().unwrap() = bound;
    let bound = run(home.path(), workspace.path(), &["policy", "sync"]);
    assert!(String::from_utf8_lossy(&bound.stdout).contains("wrong_edge"));

    payload["edge_key_id"] = json!(null);
    payload["expires_at"] = json!("2026-09-06T00:00:01Z");
    *hub.served.lock().unwrap() = signed_by(&signer, &payload);
    let expired = run(home.path(), workspace.path(), &["policy", "sync"]);
    assert!(String::from_utf8_lossy(&expired.stdout).contains("expired"));

    *hub.served.lock().unwrap() = signer.envelope(
        2,
        json!({"policy_mode": "strict", "scopes": [{"match": "", "policy_mode": "observe"}]}),
    );
    let invalid = run(home.path(), workspace.path(), &["policy", "sync"]);
    assert!(!invalid.status.success());
    assert!(
        String::from_utf8_lossy(&invalid.stdout).contains("invalid_policy"),
        "{}",
        String::from_utf8_lossy(&invalid.stdout)
    );
    assert_eq!(policy_mode(home.path()), "strict");
    assert_eq!(
        status(home.path(), workspace.path())["applied"]["revision"],
        1
    );

    // The hub goes away. The last-known-good policy governs a mediated
    // crossing with no hub in the path, and the status says the hub was
    // unreachable while the applied revision stands.
    drop(hub);
    let offline = run(home.path(), workspace.path(), &["policy", "sync"]);
    assert!(!offline.status.success());
    assert!(String::from_utf8_lossy(&offline.stdout).contains("unreachable"));
    assert_eq!(policy_mode(home.path()), "strict");
    assert!(
        mediated_fetch_refused(home.path(), &page_url),
        "enforced offline"
    );
    let offline_status = status(home.path(), workspace.path());
    assert_eq!(offline_status["desired"]["outcome"], "unreachable");
    assert_eq!(offline_status["applied"]["revision"], 1);
    assert_eq!(
        classify(&offline_status, 1, &strict_digest).0,
        Convergence::Current
    );
    let readable = run(home.path(), workspace.path(), &["status"]);
    let text = String::from_utf8_lossy(&readable.stdout);
    assert!(text.contains("deployment mode   managed"), "{text}");
    assert!(
        text.contains("desired revision  none learned; last sync unreachable"),
        "{text}"
    );

    // The key is revoked at the hub and the edge has learnt it. Signing stops,
    // and the edge names its own withdrawn identity rather than reporting the
    // hub it can no longer authenticate to as unreachable. The applied policy
    // stays in force, as it does through every other failure.
    webbotauth::revoke(home.path(), &key_id, &mut directory.lock().expect("lock"));
    let revoked = run(home.path(), workspace.path(), &["policy", "sync"]);
    assert!(!revoked.status.success());
    let text = String::from_utf8_lossy(&revoked.stdout);
    assert!(text.contains("outcome       unauthenticated"), "{text}");
    assert!(text.contains("revoked by the owner"), "{text}");
    assert_eq!(policy_mode(home.path()), "strict");
    assert_eq!(
        status(home.path(), workspace.path())["applied"]["revision"],
        1
    );
}

fn signed_by(signer: &Signer, payload: &Value) -> Vec<u8> {
    let signature = signer.pair.sign(canonical_json(payload).as_bytes());
    serde_json::to_vec(&json!({
        "envelope": ENVELOPE,
        "payload": payload,
        "signature": {"algorithm": "ed25519", "key_id": signer.key_id,
                      "value": encode_hex(signature.as_ref())},
    }))
    .expect("serialises")
}

/// The reviewer's path: the pre-image the binary prints digests to the
/// identity it records.
#[test]
fn the_printed_pre_image_recomputes_to_the_recorded_identity() {
    use sha2::{Digest, Sha256};
    let home = tempfile::tempdir().expect("tempdir");
    let workspace = tempfile::tempdir().expect("tempdir");
    std::fs::write(home.path().join("policy.json"), STRICT).expect("policy");
    let output = run(home.path(), workspace.path(), &["policy", "identity"]);
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.lines();
    let pre_image = lines.next().expect("the pre-image line");
    let digest = lines.next().expect("the digest line");
    assert_eq!(
        digest,
        format!("sha256:{:x}", Sha256::digest(pre_image.as_bytes())),
        "sha256 over the printed line is the identity"
    );
    let document = status(home.path(), workspace.path());
    assert_eq!(document["applied"]["policy_identity"]["digest"], digest);
    let parsed: Value = serde_json::from_str(pre_image).expect("the pre-image is JSON");
    assert_eq!(parsed["addons"], "unknown");
    assert_eq!(parsed["mode"], "strict");
}
