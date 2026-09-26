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

    /// The hub publishes a policy in the loader's form and digests what
    /// it publishes; a policy the loader would not load is carried as
    /// submitted, and the edge refuses it after the digest is checked.
    fn envelope(&self, revision: u64, policy: Value) -> Vec<u8> {
        let carried = serde_json::from_value::<PolicyFile>(policy.clone())
            .map(|policy| serde_json::to_value(&policy).expect("serialises"))
            .unwrap_or(policy);
        let payload = json!({
            "organisation": "org-1",
            "edge_key_id": null,
            "revision": revision,
            "issued_at": "2026-09-06T00:00:00Z",
            "expires_at": "2099-01-01T00:00:00Z",
            "policy": carried,
            "digest": canonical_digest(&carried),
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

/// A synchronisation the edge runs for itself: the session-start hook and
/// the MCP server on a host without such a hook each record what the
/// refresh did; an envelope that has expired keeps enforcing and is reported
/// stale since its own expiry by `status`, `doctor` and the session record;
/// the relay's refresh says so on its way; and the hub renewing the
/// revision clears it without a new revision.
#[test]
fn a_session_start_refreshes_the_policy_and_an_expired_envelope_is_enforced_and_reported_stale() {
    let signer = Signer::new("hub-policy-1");
    let strict: Value = serde_json::from_str(STRICT).expect("policy");
    let directory = Arc::new(Mutex::new(webbotauth::Directory::default()));
    // An envelope that expires a few seconds after it is accepted: long
    // enough for two syncs to find it valid, short enough to wait out.
    let soon = chrono::Utc::now() + chrono::Duration::seconds(8);
    let short_lived = |signer: &Signer, expires_at: chrono::DateTime<chrono::Utc>| {
        let carried = serde_json::to_value(
            serde_json::from_value::<PolicyFile>(strict.clone()).expect("loads"),
        )
        .expect("serialises");
        signed_by(
            signer,
            &json!({
                "organisation": "org-1", "edge_key_id": null, "revision": 1,
                "issued_at": "2026-09-06T00:00:00Z",
                "expires_at": expires_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                "policy": carried, "digest": canonical_digest(&carried),
            }),
        )
    };
    let hub = endpoint(short_lived(&signer, soon), Arc::clone(&directory));
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
    write_deployment(home.path(), &signer, &policy_url);
    webbotauth::enrol(
        home.path(),
        &hub.handle.url(),
        "https://hub.example",
        &mut directory.lock().expect("lock"),
    );

    // The session-start hook, as Claude Code runs it: the policy is
    // activated before the session's first crossing and the log says so.
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["hook", "session-start"])
        .env("COMMONMEASURE_HOME", home.path())
        .current_dir(workspace.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the hook starts");
    writeln!(
        child.stdin.as_mut().expect("stdin"),
        "{}",
        json!({"session_id": "managed-start", "hook_event_name": "SessionStart",
               "source": "startup", "cwd": workspace.path()})
    )
    .expect("write");
    let hook = child.wait_with_output().expect("wait");
    assert!(hook.status.success());
    assert!(
        String::from_utf8_lossy(&hook.stdout).contains("context_fetch"),
        "the nudge is still delivered"
    );
    let records = session_records(home.path(), "managed-start");
    let sync = records
        .iter()
        .find(|record| record["event"] == "policy_sync")
        .expect("the refresh is on the session record");
    assert_eq!(sync["payload"]["trigger"], "session_start");
    assert_eq!(sync["payload"]["outcome"], "accepted", "{sync}");
    assert_eq!(sync["payload"]["applied"]["revision"], 1);
    assert!(sync["payload"]["stale_since"].is_null());
    assert_eq!(
        policy_mode(home.path()),
        "strict",
        "activated at session start"
    );
    assert!(
        mediated_fetch_refused(home.path(), &page_url),
        "the session runs under the refreshed policy"
    );
    // The refresh precedes the session's first record, so the nudge
    // record names the refreshed policy, not the one it replaced.
    let nudge = records
        .iter()
        .find(|record| record["event"] == "nudge_issued")
        .expect("the nudge issuance is recorded");
    assert_eq!(
        nudge["payload"]["policy_identity"],
        status(home.path(), workspace.path())["applied"]["policy_identity"]["digest"],
        "the session start names the policy the refresh activated"
    );

    // The MCP server on a host with no session-start hook does the same at
    // its start, and says which path ran it.
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["mcp", "--host", "codex", "--session", "codex-start"])
        .env("COMMONMEASURE_HOME", home.path())
        .current_dir(workspace.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the server starts");
    drop(child.stdin.take());
    assert!(child.wait_with_output().expect("wait").status.success());
    let records = session_records(home.path(), "codex-start");
    let sync = records
        .iter()
        .find(|record| record["event"] == "policy_sync")
        .expect("the server's refresh is on the session record");
    assert_eq!(sync["payload"]["trigger"], "server_start");
    assert_eq!(sync["payload"]["outcome"], "already_applied");

    // A session the hook already refreshed is not refreshed again by the
    // server, whatever host it names; one the hook never saw is, whatever
    // host it names.
    for (host, session, refreshes) in [
        ("claude-code", "managed-start", 1),
        ("claude-code", "hookless", 1),
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
            .args(["mcp", "--host", host, "--session", session])
            .env("COMMONMEASURE_HOME", home.path())
            .current_dir(workspace.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the server starts");
        drop(child.stdin.take());
        assert!(child.wait_with_output().expect("wait").status.success());
        let count = session_records(home.path(), session)
            .iter()
            .filter(|record| record["event"] == "policy_sync")
            .count();
        assert_eq!(count, refreshes, "session {session} on host {host}");
    }

    // The envelope expires. Nothing relaxes; everything that reports the
    // applied revision says since when it has been stale.
    while chrono::Utc::now() <= soon + chrono::Duration::seconds(1) {
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let expires_at = soon.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let stale = status(home.path(), workspace.path());
    assert_eq!(stale["applied"]["revision"], 1);
    assert_eq!(stale["applied"]["expires_at"], expires_at);
    assert_eq!(stale["applied"]["stale_since"], expires_at);
    assert!(
        mediated_fetch_refused(home.path(), &page_url),
        "an expired envelope keeps enforcing"
    );
    let readable = run(home.path(), workspace.path(), &["status"]);
    let text = String::from_utf8_lossy(&readable.stdout);
    assert!(
        text.contains(&format!(
            "applied revision  1, expires {expires_at} (stale since {expires_at}"
        )),
        "{text}"
    );
    let doctor = run(home.path(), workspace.path(), &["doctor", "codex"]);
    let text = String::from_utf8_lossy(&doctor.stdout);
    assert!(
        text.contains(&format!(
            "managed policy: revision 1 in force, expires {expires_at} (stale since {expires_at}"
        )),
        "{text}"
    );
    // The hub still serves the expired envelope: refused as expired, and the
    // session record of the next start says stale.
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["mcp", "--host", "pi", "--session", "pi-stale"])
        .env("COMMONMEASURE_HOME", home.path())
        .current_dir(workspace.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the server starts");
    drop(child.stdin.take());
    assert!(child.wait_with_output().expect("wait").status.success());
    let sync = session_records(home.path(), "pi-stale")
        .into_iter()
        .find(|record| record["event"] == "policy_sync")
        .expect("recorded");
    assert_eq!(sync["payload"]["outcome"], "rejected");
    assert!(
        sync["payload"]["reason"]
            .as_str()
            .expect("reason")
            .contains("expired")
    );
    assert_eq!(sync["payload"]["stale_since"], expires_at);

    // The relay refreshes before it reads clearances, and says so when the
    // envelope is stale; with nothing cleared it then refuses to send.
    let receiver = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(|_| Response::json(201, r#"{"status":"ok","events_created":0}"#))
        .expect("spawn");
    let relayed = run(
        home.path(),
        workspace.path(),
        &["relay", "--receiver", &receiver.url()],
    );
    let stderr = String::from_utf8_lossy(&relayed.stderr);
    assert!(
        stderr.contains("policy sync rejected")
            && stderr.contains("expired")
            && stderr.contains(&format!("stale since {expires_at}")),
        "{stderr}"
    );

    // The hub renews revision 1 with a later expiry: already applied, no
    // longer stale, and the relay's refresh has nothing to say.
    *hub.served.lock().unwrap() =
        short_lived(&signer, chrono::Utc::now() + chrono::Duration::days(7));
    let renewed = run(home.path(), workspace.path(), &["policy", "sync"]);
    assert!(renewed.status.success());
    let text = String::from_utf8_lossy(&renewed.stdout);
    assert!(text.contains("outcome       already_applied"), "{text}");
    assert!(!text.contains("stale since"), "{text}");
    assert!(status(home.path(), workspace.path())["applied"]["stale_since"].is_null());
    let relayed = run(
        home.path(),
        workspace.path(),
        &["relay", "--receiver", &receiver.url()],
    );
    assert!(
        !String::from_utf8_lossy(&relayed.stderr).contains("policy sync"),
        "a fresh envelope is refreshed quietly: {}",
        String::from_utf8_lossy(&relayed.stderr)
    );
}

fn session_records(home: &Path, session: &str) -> Vec<Value> {
    let path = home.join("sessions").join(format!("{session}.ndjson"));
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("ndjson"))
        .collect()
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

/// A hub that accepts the connection and never answers costs a session
/// start its budget and nothing else: the hook exits inside it with the
/// nudge delivered, the server starts inside it with the refresh recorded
/// as unreachable, and the policy in force keeps governing.
#[test]
fn a_hub_that_accepts_and_never_answers_costs_a_session_start_its_budget_and_nothing_else() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let hub_url = format!("http://{}", listener.local_addr().expect("address"));
    let policy_url = format!("{hub_url}/api/v1/policy/desired");
    // Accepted connections are held open, so the client waits rather than
    // being reset.
    let held: Arc<Mutex<Vec<std::net::TcpStream>>> = Arc::new(Mutex::new(Vec::new()));
    let keep = Arc::clone(&held);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            keep.lock().expect("lock").push(stream);
        }
    });
    let signer = Signer::new("hub-policy-1");
    let directory = Arc::new(Mutex::new(webbotauth::Directory::default()));
    let home = tempfile::tempdir().expect("tempdir");
    let workspace = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        home.path().join("policy.json"),
        r#"{"policy_mode":"strict","allow_private_hosts":true}"#,
    )
    .expect("policy");
    write_deployment(home.path(), &signer, &policy_url);
    webbotauth::enrol(
        home.path(),
        &hub_url,
        "https://hub.example",
        &mut directory.lock().expect("lock"),
    );
    let budget = commonmeasure_harness::managed::SESSION_START_BUDGET;
    let allowance = budget + std::time::Duration::from_secs(3);

    let started = std::time::Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["hook", "session-start"])
        .env("COMMONMEASURE_HOME", home.path())
        .current_dir(workspace.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the hook starts");
    writeln!(
        child.stdin.as_mut().expect("stdin"),
        "{}",
        json!({"session_id": "hanging-hub", "hook_event_name": "SessionStart",
               "source": "startup", "cwd": workspace.path()})
    )
    .expect("write");
    let hook = child.wait_with_output().expect("wait");
    let elapsed = started.elapsed();
    assert!(hook.status.success());
    assert!(
        elapsed < allowance,
        "the hook waited {elapsed:?} for a hub that never answered"
    );
    assert!(String::from_utf8_lossy(&hook.stdout).contains("context_fetch"));
    let sync = session_records(home.path(), "hanging-hub")
        .into_iter()
        .find(|record| record["event"] == "policy_sync")
        .expect("the refresh is recorded");
    assert_eq!(sync["payload"]["outcome"], "unreachable", "{sync}");
    assert!(
        sync["payload"]["reason"]
            .as_str()
            .expect("reason")
            .contains("did not answer within"),
        "{sync}"
    );

    let started = std::time::Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["mcp", "--host", "codex", "--session", "hanging-codex"])
        .env("COMMONMEASURE_HOME", home.path())
        .current_dir(workspace.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the server starts");
    drop(child.stdin.take());
    assert!(child.wait_with_output().expect("wait").status.success());
    assert!(started.elapsed() < allowance);
    let sync = session_records(home.path(), "hanging-codex")
        .into_iter()
        .find(|record| record["event"] == "policy_sync")
        .expect("the refresh is recorded");
    assert_eq!(sync["payload"]["trigger"], "server_start");
    assert_eq!(sync["payload"]["outcome"], "unreachable");
    assert_eq!(policy_mode(home.path()), "strict", "nothing relaxed");
    drop(held);
}

/// A deployment whose `policy_url` carries userinfo and is refused (plain
/// http off the machine), in both spellings the parser accepts. The refusal
/// is the session record's `reason`, which the console serves, so it names
/// the policy URL by its origin.
#[test]
fn a_refused_policy_url_is_recorded_by_its_origin() {
    let signer = Signer::new("hub-policy-1");
    for (session, policy_url) in [
        (
            "refused-slashes",
            "http://op:ak_PLANTED@hub.example/api/v1/policy/desired",
        ),
        (
            "refused-slashless",
            "http:/op:ak_PLANTED@hub.example/api/v1/policy/desired",
        ),
    ] {
        let home = tempfile::tempdir().expect("tempdir");
        let workspace = tempfile::tempdir().expect("tempdir");
        write_deployment(home.path(), &signer, policy_url);
        let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
            .args(["hook", "session-start"])
            .env("COMMONMEASURE_HOME", home.path())
            .current_dir(workspace.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the hook starts");
        writeln!(
            child.stdin.as_mut().expect("stdin"),
            "{}",
            json!({"session_id": session, "hook_event_name": "SessionStart",
                   "source": "startup", "cwd": workspace.path()})
        )
        .expect("write");
        let hook = child.wait_with_output().expect("wait");
        assert!(hook.status.success());
        let sync = session_records(home.path(), session)
            .into_iter()
            .find(|record| record["event"] == "policy_sync")
            .expect("the refusal is recorded");
        assert_eq!(sync["payload"]["outcome"], "unavailable", "{sync}");
        let reason = sync["payload"]["reason"].as_str().expect("reason");
        assert!(reason.contains("http://hub.example"), "{reason}");
        assert!(
            !reason.contains("PLANTED") && !reason.contains("op:"),
            "{reason}"
        );
    }
}

/// Drive the signed transport and policy save with independently pinned
/// request and response times. The endpoint must authenticate the request
/// before the clock supplies its response-time reading.
fn sync_at_times(
    started: chrono::DateTime<chrono::Utc>,
    arrived: chrono::DateTime<chrono::Utc>,
    issued: chrono::DateTime<chrono::Utc>,
    expires: chrono::DateTime<chrono::Utc>,
) -> commonmeasure_harness::managed::SyncReport {
    let signer = Signer::new("hub-policy-clock");
    let mut envelope: Value =
        serde_json::from_slice(&signer.envelope(1, serde_json::from_str(STRICT).expect("policy")))
            .expect("envelope");
    envelope["payload"]["issued_at"] = json!(issued.to_rfc3339());
    envelope["payload"]["expires_at"] = json!(expires.to_rfc3339());
    let directory = Arc::new(Mutex::new(webbotauth::Directory::default()));
    let hub = endpoint(
        signed_by(&signer, &envelope["payload"]),
        Arc::clone(&directory),
    );
    let home = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        home.path().join("policy.json"),
        r#"{"policy_mode":"observe"}"#,
    )
    .expect("policy");
    write_deployment(
        home.path(),
        &signer,
        &format!("{}/api/v1/policy/desired", hub.handle.url()),
    );
    let key_id = webbotauth::enrol(
        home.path(),
        &hub.handle.url(),
        "https://hub.example",
        &mut directory.lock().expect("lock"),
    );
    let mut readings = 0;
    let report = commonmeasure_harness::managed::sync(
        home.path(),
        &commonmeasure_harness::fleet::EdgeIdentity::KeyId(key_id),
        || {
            readings += 1;
            match readings {
                1 => {
                    assert_eq!(hub.requests.load(Ordering::SeqCst), 0);
                    started
                }
                2 => {
                    assert_eq!(hub.authenticated.lock().expect("lock").len(), 1);
                    arrived
                }
                _ => panic!("unexpected clock reading"),
            }
        },
    )
    .expect("sync");
    assert_eq!(
        readings, 2,
        "validity needs a fresh clock reading after the fetch"
    );
    if report.sync.outcome == "accepted" {
        assert_eq!(policy_mode(home.path()), "strict");
        assert_eq!(report.applied.as_ref().expect("applied").revision, 1);
        assert_eq!(
            std::fs::read(commonmeasure_harness::managed::State::last_known_good_path(
                home.path()
            ))
            .expect("saved envelope"),
            *hub.served.lock().expect("lock")
        );
    } else {
        assert_eq!(
            policy_mode(home.path()),
            "observe",
            "a refusal preserves local policy"
        );
        assert!(report.applied.is_none());
    }
    report
}

#[test]
fn an_envelope_issued_during_the_request_is_valid_when_the_response_arrives() {
    let started = chrono::Utc::now();
    let arrived = started + chrono::Duration::seconds(1);
    let report = sync_at_times(
        started,
        arrived,
        arrived,
        arrived + chrono::Duration::hours(1),
    );
    assert_eq!(report.sync.outcome, "accepted", "{:?}", report.sync);
}

#[test]
fn an_envelope_issued_within_the_clock_skew_tolerance_is_accepted() {
    use commonmeasure_harness::managed::ISSUED_AT_TOLERANCE;
    let now = chrono::Utc::now();
    for offset in [chrono::Duration::seconds(1), ISSUED_AT_TOLERANCE] {
        let report = sync_at_times(now, now, now + offset, now + chrono::Duration::hours(1));
        assert_eq!(report.sync.outcome, "accepted", "{:?}", report.sync);
    }
}

#[test]
fn an_envelope_beyond_the_clock_skew_tolerance_is_rejected_with_the_tolerance() {
    use commonmeasure_harness::managed::ISSUED_AT_TOLERANCE;
    let now = chrono::Utc::now();
    let report = sync_at_times(
        now,
        now,
        now + ISSUED_AT_TOLERANCE + chrono::Duration::nanoseconds(1),
        now + chrono::Duration::hours(1),
    );
    assert_eq!(report.sync.outcome, "rejected");
    let reason = report.sync.reason.expect("reason");
    assert!(reason.ends_with("(not_yet_valid)"), "{reason}");
    assert!(
        reason.contains(&format!(
            "{} second clock-skew tolerance",
            ISSUED_AT_TOLERANCE.num_seconds()
        )),
        "{reason}"
    );
}

#[test]
fn an_expired_envelope_has_no_clock_skew_tolerance() {
    let started = chrono::Utc::now();
    let arrived = started + chrono::Duration::seconds(1);
    for expires in [arrived - chrono::Duration::nanoseconds(1), arrived] {
        let report = sync_at_times(
            started,
            arrived,
            started - chrono::Duration::hours(1),
            expires,
        );
        assert_eq!(report.sync.outcome, "rejected");
        assert_eq!(
            report.sync.reason.expect("reason"),
            format!(
                "the envelope expired at {} (expired)",
                expires.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
            )
        );
    }
}
