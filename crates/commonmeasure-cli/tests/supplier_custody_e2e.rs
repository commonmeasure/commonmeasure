//! Supplier credential custody driven as it is deployed: the real binary
//! under `hosted service` against a running Common Measure Hub
//! (`docs/contracts/supplier-credentials.md`). The offline tests are
//! `crates/commonmeasure-relay/tests/supplier_credentials.rs` and the custody
//! test in `crates/commonmeasure-cli/tests/hosted_service.rs`, both against a
//! test double of the release route; this file holds the one test against a
//! real hub, ignored by default. See
//! `a_real_hub_releases_rotates_withdraws_and_revokes` for how to run it.
//!
//! First run on 19 September 2026 against a hub build on
//! loopback (the hub server and a PostgreSQL database made for the run): it
//! passed as written, so the owner routes it calls, read from the hub's work
//! in progress, match the hub as built. No transcript is kept.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use commonmeasure_http::{Request, send};
use serde_json::{Value, json};

/// Made-up values. Neither is a supplier's key and no supplier is called.
const FIRST: &str = "custody-e2e-5d0e77b1-first-value";
const ROTATED: &str = "custody-e2e-5d0e77b1-rotated-value";

/// The service's interval in this run, and so its revocation bound less the
/// request budget.
const INTERVAL_SECONDS: u64 = 2;

struct Service(Child);

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn commonmeasure(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(args)
        .env("COMMONMEASURE_HOME", home)
        .output()
        .expect("the binary should run")
}

fn everything_under(root: &Path) -> String {
    let mut text = String::new();
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("readable") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                text.push_str(&String::from_utf8_lossy(
                    &std::fs::read(&path).expect("read"),
                ));
            }
        }
    }
    text
}

/// Custody against a running Common Measure Hub: an enrolled, managed edge
/// runs the hosted service with `supplier_custody` set; the owner connects an
/// Exa account with a made-up value, authorises this edge, rotates, withdraws,
/// authorises again and revokes; after each act the service's fetch record
/// shows it within one interval, with no restart. Ignored by default because
/// it needs a hub. To run it:
///
/// 1. Run a Common Measure Hub server that has the supplier credential
///    custody slice, with its database migrated, `IDENTITY_ORIGIN` set and
///    its public origin equal to the URL the edge is given, with an
///    organisation whose owner has a session.
/// 2. Export `COMMONMEASURE_TEST_HUB` (the server's base URL, e.g.
///    `http://127.0.0.1:8080`) and `COMMONMEASURE_TEST_HUB_SESSION` (the
///    owner's raw session token, sent as the `__Host-session` cookie).
/// 3. `cargo test -p commonmeasure-cli --test supplier_custody_e2e -- --ignored --nocapture`
///
/// It prints a transcript that carries no value. It establishes release,
/// rotation, withdrawal and revocation as the edge sees them, and the bound
/// they arrive in. It opens no hosted session (that needs an access token
/// from the hub's authorisation server) and makes no search, so it
/// establishes nothing about Exa or about the contract's acceptance case 1.
#[test]
#[ignore = "requires a running hub server with the custody slice (see the doc comment)"]
fn a_real_hub_releases_rotates_withdraws_and_revokes() {
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
    let owner = |method: &str, path: &str, body: Option<Value>| -> (u16, Value) {
        let mut request = match body {
            Some(body) => Request::post(path, body.to_string().into_bytes(), "application/json"),
            None => Request::get(path),
        };
        request.method = method.to_owned();
        request.headers.set("Cookie", &cookie);
        let response = send(&format!("{hub_url}{path}"), request).expect("the hub answers");
        (
            response.status,
            serde_json::from_slice(&response.body).unwrap_or(Value::Null),
        )
    };

    println!("## 1. The owner publishes a policy and mints a token; the edge connects as managed");
    let (status, minted) = owner(
        "POST",
        "/api/v1/enrolment/tokens",
        Some(json!({"name": "custody-run"})),
    );
    assert_eq!(status, 201, "{minted}");
    let token = minted["value"].as_str().expect("token").to_owned();
    let (status, published) = owner(
        "POST",
        "/api/v1/policy/revisions",
        Some(json!({"policy": {"policy_mode": "observe"}})),
    );
    assert_eq!(status, 201, "{published}");
    let connected = commonmeasure(
        home.path(),
        &["connect", &hub_url, "--token", &token, "--managed"],
    );
    assert!(
        connected.status.success(),
        "{}",
        String::from_utf8_lossy(&connected.stderr)
    );
    let key_id = commonmeasure_harness::enrolled_key_id(home.path())
        .expect("a readable enrolment record")
        .expect("an enrolled key id");
    println!("connected; key id {key_id}");

    println!("\n## 2. The owner connects a supplier account; the value is in no answer");
    let (status, created) = owner(
        "POST",
        "/api/v1/supplier-connections",
        Some(json!({"provider": "exa", "label": "custody run", "value": FIRST})),
    );
    assert_eq!(status, 201, "{created}");
    assert!(!created.to_string().contains(FIRST));
    let connection = created["id"].as_str().expect("a connection id").to_owned();
    println!("POST /api/v1/supplier-connections -> 201, connection {connection}");

    println!("\n## 3. The service starts with custody configured and holds nothing yet");
    std::fs::write(
        home.path().join("hosted-service.json"),
        json!({
            "listen": "127.0.0.1:0", "origin": "https://edge.test",
            "hosts": ["claude-connector"], "interval_seconds": INTERVAL_SECONDS,
            "supplier_custody": true,
        })
        .to_string(),
    )
    .unwrap();
    let journal_path = home.path().join("journal.txt");
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["hosted", "service"])
        .env("COMMONMEASURE_HOME", home.path())
        .env_remove("EXA_API_KEY")
        .stdout(Stdio::piped())
        .stderr(Stdio::from(std::fs::File::create(&journal_path).unwrap()))
        .spawn()
        .expect("the service should start");
    let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();
    let _service = Service(child);
    loop {
        let line = lines
            .next()
            .expect("the service announces its address")
            .unwrap();
        if line.starts_with("listening on ") {
            break;
        }
    }
    let record = || -> Value {
        std::fs::read_to_string(home.path().join("supplier-credentials.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null)
    };
    // One interval and one request budget is the contract's bound; the wait
    // allows that and fails past it.
    let bound = Duration::from_secs(INTERVAL_SECONDS) + commonmeasure_http::CLIENT_TIMEOUT;
    let within_bound = |what: &str, condition: &dyn Fn(&Value) -> bool| {
        let started = Instant::now();
        loop {
            let now = record();
            if condition(&now) {
                println!(
                    "{what}: outcome={} http_status={} held={} after {:.1}s (bound {}s)",
                    now["outcome"],
                    now["http_status"],
                    now["held"],
                    started.elapsed().as_secs_f32(),
                    bound.as_secs()
                );
                return;
            }
            assert!(
                started.elapsed() < bound,
                "{what} did not arrive within the bound: {now}"
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    };
    let holds_nothing = |now: &Value| now["http_status"] == 200 && now["held"] == json!([]);
    let holds = |now: &Value| now["held"][0]["connection_id"] == connection.as_str();
    within_bound("before authorisation", &holds_nothing);

    println!("\n## 4. Authorise, rotate, withdraw, authorise again, revoke");
    let edge_path = format!("/api/v1/supplier-connections/{connection}/edges/{key_id}");
    let (status, answer) = owner("PUT", &edge_path, None);
    assert!(status < 300, "{status} {answer}");
    within_bound("authorised", &holds);

    let (status, answer) = owner(
        "POST",
        &format!("/api/v1/supplier-connections/{connection}/rotate"),
        Some(json!({"value": ROTATED})),
    );
    assert!(status < 300, "{status} {answer}");
    within_bound("rotated", &|now| {
        holds(now) && now["held"][0]["rotated_at"].is_string()
    });

    let (status, answer) = owner("DELETE", &edge_path, None);
    assert!(status < 300, "{status} {answer}");
    within_bound("withdrawn", &holds_nothing);

    let (status, answer) = owner("PUT", &edge_path, None);
    assert!(status < 300, "{status} {answer}");
    within_bound("authorised again", &holds);
    let (status, answer) = owner(
        "DELETE",
        &format!("/api/v1/supplier-connections/{connection}"),
        None,
    );
    assert!(status < 300, "{status} {answer}");
    within_bound("revoked", &holds_nothing);

    println!("\n## 5. Neither value is on the machine or in the journal");
    let written = everything_under(home.path());
    for value in [FIRST, ROTATED] {
        assert!(
            !written.contains(value),
            "a released value was written under the home"
        );
    }
    println!("{}", std::fs::read_to_string(&journal_path).unwrap());
}
