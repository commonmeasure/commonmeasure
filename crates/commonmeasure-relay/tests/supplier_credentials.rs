//! The supplier credential release client against a **test double of the
//! hub's release route** (`docs/contracts/supplier-credentials.md` §Release).
//!
//! The double is a loopback `commonmeasure_http::Server`, not the hub. The
//! hub half of the contract was being built while this was written, so the
//! double is written from the contract and from the hub's specification of
//! the edge's request signature, independent
//! of the edge's signing code: the headers agree on a label, the signature is
//! tagged `web-bot-auth` and names Ed25519, it covers `@authority` and, by
//! the contract's §Signature components of the release request, `@method` and
//! `@path`, which the double reads from the request as it arrived; its window
//! is at most 360 seconds and contains now with one minute of skew, the
//! `keyid` resolves an enrolled key (from the double's own table, as the hub
//! resolves `edge_keys`, not from a key directory), the Ed25519 signature
//! verifies over the rebuilt RFC 9421 base, and the nonce is at least 22
//! characters, is stored, and a repeat is refused. It refuses a request
//! carrying `Authorization`, since the ingest key must not authenticate the
//! route. It answers the contract's `200`
//! document, an empty list for an edge with no authorisation, and `401` for a
//! key it does not hold or has revoked. Each refusal is `{detail, code}` with
//! the hub's stable codes (its table of `401` codes; its `400`
//! `query_refused` likewise), though the double does
//! not tell `authority_mismatch` from `signature_invalid`. It has no
//! database, no tenancy policy, no rate limiter and no audit log; a forced
//! answer stands in for a `404`, a `429` and malformed `200`s, and a queued
//! `401` code for a fault the double cannot produce on demand (a repeated
//! nonce, a signature the hub could not verify), each injected to exercise
//! the client, establishing nothing about the hub. One test uses a second
//! peer that is not the double: a raw socket answering a misframed `200`,
//! which verifies nothing and stands for a broken intermediary.
//!
//! What these tests establish: the bytes the client puts on the wire pass
//! that rule, the client does what the contract says with each answer, and
//! a released value reaches adapter construction and nothing else. They do
//! not establish integration with the hub; the ignored test in
//! `crates/commonmeasure-cli/tests/supplier_custody_e2e.rs` is the one that
//! runs against a real hub. No test here calls a supplier.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use commonmeasure_harness::enrolment::{EnrolledIdentity, EnrolledOrganization, EnrolmentRecord};
use commonmeasure_harness::identity::{Derived, EdgeKey, Identity, SigningIdentity};
use commonmeasure_harness::mcp::McpServer;
use commonmeasure_harness::policy::SessionPolicy;
use commonmeasure_harness::session::SessionLog;
use commonmeasure_http::{Request, Response, Server, ServerHandle};
use commonmeasure_relay::supplier_credentials::{self as client, BUDGET, Fetch, Outcome};
use commonmeasure_supply::SupplyError;
use commonmeasure_supply::credentials::{CredentialsStatus, ReleasedStore};
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::{Value, json};

/// Values no test may find anywhere but in the double's own table.
const FIRST_VALUE: &str = "exa-released-7f3a9c52-first";
const ROTATED_VALUE: &str = "exa-released-7f3a9c52-rotated";

struct Connection {
    id: &'static str,
    organisation: &'static str,
    provider: &'static str,
    variable: &'static str,
    value: String,
    rotated_at: Option<String>,
    revoked: bool,
    /// The enrolled keys this connection is authorised to.
    authorised: HashSet<String>,
}

#[derive(Default)]
struct Double {
    authority: String,
    /// Enrolled keys: key id to public key and organisation.
    keys: HashMap<String, ([u8; 32], &'static str)>,
    connections: Vec<Connection>,
    nonces: HashSet<(String, String)>,
    /// Keys revoked at the hub: a request signed by one is `key_revoked`.
    revoked: HashSet<String>,
    /// Answer every request with this, whatever it carries.
    forced: Option<(u16, String)>,
    /// Answer each of the next requests `401` with the next code, before
    /// looking at its signature.
    faults: VecDeque<&'static str>,
    /// Hold each answer this long before sending it.
    delay: std::time::Duration,
    before_answer: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Each request as it arrived, for replaying one.
    received: Vec<Request>,
    /// What an audit row would name for each `200`: key id and connections.
    released: Vec<(String, Vec<&'static str>)>,
}

/// A refusal as the hub answers one: a stable `code` beside a `detail`.
fn refusal(status: u16, code: &str) -> Response {
    let detail = match code {
        "query_refused" => "the supplier credential release takes no query string".to_owned(),
        code => {
            format!("this endpoint authenticates an enrolled edge by its request signature; {code}")
        }
    };
    Response::json(status, &json!({"detail": detail, "code": code}).to_string())
}

/// One `;name="value"` signature parameter.
fn quoted(params: &str, name: &str) -> Option<String> {
    let at = params.find(&format!(";{name}=\""))? + name.len() + 3;
    let rest = &params[at..];
    Some(rest[..rest.find('"')?].to_owned())
}

/// One `;name=integer` signature parameter.
fn integer(params: &str, name: &str) -> Option<i64> {
    let at = params.find(&format!(";{name}="))? + name.len() + 2;
    let rest = &params[at..];
    rest[..rest.find(';').unwrap_or(rest.len())].parse().ok()
}

impl Double {
    /// The rule of `auth.md` §The edge's request signature, plus what the
    /// custody contract adds for this route: the stored nonce, the covered
    /// `@method` and `@path`, and the 360-second cap on the window. Any
    /// departure is `401`.
    fn authenticate(&mut self, request: &Request) -> Result<String, &'static str> {
        const INVALID: &str = "signature_invalid";
        if request.headers.get("authorization").is_some() {
            return Err(INVALID);
        }
        let field = |name: &str| request.headers.get(name).ok_or(INVALID);
        let (label, params) = field("signature-input")?.split_once('=').ok_or(INVALID)?;
        let (signature_label, encoded) = field("signature")?.split_once('=').ok_or(INVALID)?;
        if label != signature_label
            || !params.contains(r#";tag="web-bot-auth""#)
            || !params.contains(r#";alg="ed25519""#)
        {
            return Err(INVALID);
        }
        let covered: Vec<&str> = params
            .strip_prefix('(')
            .and_then(|rest| rest.split_once(')'))
            .map(|(inside, _)| inside.split_whitespace().collect())
            .ok_or(INVALID)?;
        if !covered.contains(&"\"@authority\"") {
            return Err(INVALID);
        }
        if !["\"@method\"", "\"@path\""]
            .iter()
            .all(|required| covered.contains(required))
        {
            return Err("components_missing");
        }
        let now = Utc::now().timestamp();
        let (created, expires) = (
            integer(params, "created").ok_or(INVALID)?,
            integer(params, "expires").ok_or(INVALID)?,
        );
        if expires <= created {
            return Err(INVALID);
        }
        if created > now + 60 || expires < now - 60 {
            return Err("signature_expired");
        }
        if expires - created > 360 {
            return Err("window_too_long");
        }
        let key_id = quoted(params, "keyid").ok_or(INVALID)?;
        let nonce = quoted(params, "nonce")
            .filter(|nonce| nonce.len() >= 22)
            .ok_or("nonce_missing")?;

        let mut base = String::new();
        for name in &covered {
            let name = name.trim_matches('"');
            // The derived components come from the request as it arrived,
            // never from anything the signer sent about itself.
            let value = match name {
                "@authority" => self.authority.clone(),
                "@method" => request.method.clone(),
                "@path" => request
                    .target
                    .split_once('?')
                    .map_or(request.target.as_str(), |(path, _)| path)
                    .to_owned(),
                header => field(header)?.to_owned(),
            };
            base.push_str(&format!("\"{name}\": {value}\n"));
        }
        base.push_str(&format!("\"@signature-params\": {params}"));
        let signature: [u8; 64] = STANDARD
            .decode(encoded.trim().trim_matches(':'))
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or(INVALID)?;
        let (public, _) = self.keys.get(&key_id).ok_or("key_unknown")?;
        VerifyingKey::from_bytes(public)
            .map_err(|_| INVALID)?
            .verify_strict(base.as_bytes(), &Signature::from_bytes(&signature))
            .map_err(|_| INVALID)?;
        if self.revoked.contains(&key_id) {
            return Err("key_revoked");
        }
        if !self.nonces.insert((key_id.clone(), nonce)) {
            return Err("nonce_repeated");
        }
        Ok(key_id)
    }

    fn handle(&mut self, request: &Request) -> Response {
        self.received.push(request.clone());
        let (path, query) = request
            .target
            .split_once('?')
            .map_or((request.target.as_str(), None), |(path, query)| {
                (path, Some(query))
            });
        if request.method != "GET" || path != client::RELEASE_PATH {
            return Response::json(404, r#"{"detail":"not found"}"#);
        }
        if query.is_some() {
            return refusal(400, "query_refused");
        }
        if let Some((status, body)) = &self.forced {
            return Response::json(*status, body);
        }
        if let Some(code) = self.faults.pop_front() {
            return refusal(401, code);
        }
        let key_id = match self.authenticate(request) {
            Ok(key_id) => key_id,
            Err(reason) => return refusal(401, reason),
        };
        let organisation = self.keys[&key_id].1;
        let served: Vec<&Connection> = self
            .connections
            .iter()
            .filter(|connection| {
                connection.organisation == organisation
                    && !connection.revoked
                    && connection.authorised.contains(&key_id)
            })
            .collect();
        self.released.push((
            key_id.clone(),
            served.iter().map(|connection| connection.id).collect(),
        ));
        let mut response = Response::json(
            200,
            &json!({
                "organisation": organisation,
                "key_id": key_id,
                "served_at": Utc::now().to_rfc3339(),
                "credentials": served.iter().map(|connection| json!({
                    "connection_id": connection.id,
                    "provider": connection.provider,
                    "variable": connection.variable,
                    "value": connection.value,
                    "rotated_at": connection.rotated_at,
                })).collect::<Vec<_>>(),
            })
            .to_string(),
        );
        response.headers.set("Cache-Control", "no-store");
        response.headers.set("Pragma", "no-cache");
        response
    }

    fn connection(&mut self, id: &str) -> &mut Connection {
        self.connections
            .iter_mut()
            .find(|connection| connection.id == id)
            .expect("a connection the test created")
    }
}

/// Enrol `home` with the double under `organisation` as a managed edge, as
/// `connect --managed` leaves it, and return its key id.
fn enrol(
    home: &Path,
    double: &Arc<Mutex<Double>>,
    url: &str,
    organisation: &'static str,
) -> String {
    let key = EdgeKey::generate().unwrap();
    key.store(home).unwrap();
    let key_id = key.thumbprint();
    let public: [u8; 32] = URL_SAFE_NO_PAD
        .decode(key.jwk_x())
        .unwrap()
        .try_into()
        .unwrap();
    EnrolmentRecord {
        hub: url.to_owned(),
        organization: EnrolledOrganization {
            id: organisation.to_owned(),
            name: "Org".to_owned(),
        },
        name: "hosted-edge".to_owned(),
        key_id: key_id.clone(),
        identity: EnrolledIdentity {
            origin: "https://hub.example".to_owned(),
            bot_page: "https://hub.example/bot".to_owned(),
            contact: None,
        },
        enrolled_at: "2026-09-19T00:00:00.000Z".to_owned(),
        revoked_at: None,
        revocation: None,
        revocation_learnt_at: None,
    }
    .store(home)
    .unwrap();
    write_deployment(home, &format!("{url}/api/v1/policy/desired"), organisation);
    double
        .lock()
        .unwrap()
        .keys
        .insert(key_id.clone(), (public, organisation));
    key_id
}

fn write_deployment(home: &Path, policy_url: &str, organisation: &str) {
    std::fs::write(
        home.join("deployment.json"),
        json!({
            "mode": "managed", "organisation": organisation, "policy_url": policy_url,
            "signer": {"key_id": "policy-1", "algorithm": "ed25519", "public_key": "00".repeat(32)},
        })
        .to_string(),
    )
    .unwrap();
}

/// The double, serving, with one Exa connection of `org-1` authorised to
/// nobody yet.
fn double() -> (Arc<Mutex<Double>>, ServerHandle) {
    let server = Server::bind("127.0.0.1:0").unwrap();
    let double = Arc::new(Mutex::new(Double {
        authority: server.local_addr().unwrap().to_string(),
        connections: vec![Connection {
            id: "conn-exa-1",
            organisation: "org-1",
            provider: "exa",
            variable: "EXA_API_KEY",
            value: FIRST_VALUE.to_owned(),
            rotated_at: None,
            revoked: false,
            authorised: HashSet::new(),
        }],
        ..Double::default()
    }));
    let state = Arc::clone(&double);
    let handle = server
        .spawn(move |request| {
            let (response, delay, before_answer) = {
                let mut double = state.lock().unwrap();
                (
                    double.handle(&request),
                    double.delay,
                    double.before_answer.clone(),
                )
            };
            if let Some(before_answer) = before_answer {
                before_answer();
            }
            std::thread::sleep(delay);
            response
        })
        .unwrap();
    (double, handle)
}

/// An enrolled home of `org-1` whose key the Exa connection is authorised to.
fn authorised_edge() -> (tempfile::TempDir, Arc<Mutex<Double>>, ServerHandle, String) {
    let (double, handle) = double();
    let home = tempfile::tempdir().unwrap();
    let key_id = enrol(home.path(), &double, &handle.url(), "org-1");
    double
        .lock()
        .unwrap()
        .connection("conn-exa-1")
        .authorised
        .insert(key_id.clone());
    (home, double, handle, key_id)
}

/// A store as the hosted service builds one at the default interval.
fn store() -> ReleasedStore {
    client::store_for(std::time::Duration::from_secs(300))
}

fn fetch(home: &Path, store: &ReleasedStore) -> Fetch {
    client::fetch(home, store, Utc::now(), BUDGET).expect("the fetch is attempted and recorded")
}

/// Every file under `root`, as text.
fn everything_under(root: &Path) -> String {
    let mut text = String::new();
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                text.push_str(&String::from_utf8_lossy(&std::fs::read(&path).unwrap()));
            }
        }
    }
    text
}

fn assert_no_value(what: &str, text: &str) {
    for value in [FIRST_VALUE, ROTATED_VALUE] {
        assert!(!text.contains(value), "{what} carries a released value");
    }
}

/// Whether the machine running the tests exports Exa's variable. The
/// environment wins over a release, so the assertions about use differ.
fn exa_in_environment() -> bool {
    std::env::var("EXA_API_KEY").is_ok_and(|value| !value.trim().is_empty())
}

// Catches: a request the hub's signature rule refuses; a repeated nonce; a
// release not held, or held under another name; a value written to the fetch
// record or rendered by a debug or journal line; a refresh that re-reports a
// change when nothing changed or fails to advance the fetch time.
#[test]
fn an_authorised_edge_is_released_its_key_and_a_refresh_is_unchanged() {
    let (home, double, handle, key_id) = authorised_edge();
    let store = store();

    let first = fetch(home.path(), &store);
    assert_eq!(first.outcome, Outcome::Accepted, "{first:?}");
    assert_eq!(first.http_status, Some(200));
    assert_eq!(
        first.url,
        format!("{}{}", handle.url(), client::RELEASE_PATH)
    );
    assert_eq!(first.held.len(), 1);
    assert_eq!(first.held[0].connection_id, "conn-exa-1");
    assert_eq!(first.held[0].variable, "EXA_API_KEY");
    assert_eq!(store.names()[0].provider, "exa");

    let second = client::fetch(
        home.path(),
        &store,
        Utc::now() + Duration::seconds(1),
        BUDGET,
    )
    .expect("recorded");
    assert_eq!(second.outcome, Outcome::Unchanged, "{second:?}");
    assert!(second.held[0].fetched_at > first.held[0].fetched_at);
    assert_eq!(Fetch::read(home.path()).unwrap(), Some(second.clone()));

    // Two requests, two nonces, both admitted; the audit the double would
    // write names the key and the connection.
    {
        let double = double.lock().unwrap();
        assert_eq!(double.nonces.len(), 2);
        assert_eq!(
            double.released,
            vec![(key_id.clone(), vec!["conn-exa-1"]); 2]
        );
        let sent = &double.received[0];
        assert!(sent.headers.get("authorization").is_none());
        assert!(sent.headers.get("x-api-key").is_none());
        let input = sent.headers.get("signature-input").unwrap();
        assert!(
            input
                .starts_with(r#"sig1=("@authority" "signature-agent" "@method" "@path");created="#),
            "{input}"
        );
    }
    // The double does refuse a repeat, so the two admissions above show the
    // client signs a fresh nonce each time.
    let captured = double.lock().unwrap().received[0].clone();
    let replayed = commonmeasure_http::send(
        &format!("{}{}", handle.url(), client::RELEASE_PATH),
        captured,
    )
    .unwrap();
    assert_eq!(replayed.status, 401);
    assert_eq!(
        serde_json::from_slice::<Value>(&replayed.body).unwrap()["code"],
        "nonce_repeated"
    );

    assert_no_value("the operator home", &everything_under(home.path()));
    assert_no_value(
        "a rendering",
        &format!("{first:?} {second:?} {store:?} {}", second.journal_line()),
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(Fetch::path(home.path()))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "the fetch record is owner readable only");
    }
}

// Catches: a revoked or withdrawn credential still supplied after the fetch
// that learnt of it; the unavailable error naming anything but the variable;
// an empty release treated as a failure rather than the withdrawn state.
#[test]
fn revocation_and_withdrawal_reach_the_edge_at_its_next_fetch() {
    let (home, double, _handle, key_id) = authorised_edge();
    let store = store();
    fetch(home.path(), &store);
    assert_eq!(store.names().len(), 1);
    if !exa_in_environment() {
        assert!(commonmeasure_supply::supplier_with_released("exa", Some(&store)).is_ok());
    }

    double.lock().unwrap().connection("conn-exa-1").revoked = true;
    let after = fetch(home.path(), &store);
    assert_eq!(after.outcome, Outcome::Accepted, "{after:?}");
    assert_eq!(after.http_status, Some(200));
    assert!(after.held.is_empty());
    assert!(store.names().is_empty());
    if !exa_in_environment() {
        let error = commonmeasure_supply::supplier_with_released("exa", Some(&store))
            .err()
            .expect("nothing supplies the variable");
        assert_eq!(
            error,
            SupplyError::CredentialMissing {
                variable: "EXA_API_KEY".to_owned()
            }
        );
    }

    // Withdrawal reads the same from the edge: reinstate, release, withdraw.
    {
        let mut double = double.lock().unwrap();
        double.connection("conn-exa-1").revoked = false;
    }
    assert_eq!(fetch(home.path(), &store).held.len(), 1);
    double
        .lock()
        .unwrap()
        .connection("conn-exa-1")
        .authorised
        .remove(&key_id);
    let withdrawn = fetch(home.path(), &store);
    assert_eq!(withdrawn.outcome, Outcome::Accepted);
    assert!(store.names().is_empty());
    assert_eq!(fetch(home.path(), &store).outcome, Outcome::Unchanged);
}

// Catches: a rotation needing a new store or a restart; a rotation changing
// the connection the records name; a rotated value reported as unchanged.
#[test]
fn a_rotation_reaches_the_same_store_under_the_same_connection() {
    let (home, double, _handle, _) = authorised_edge();
    let store = store();
    // A session opened before the rotation reads this handle.
    let session_handle = store.clone();
    fetch(home.path(), &store);
    assert_eq!(session_handle.names()[0].rotated_at, None);

    {
        let mut double = double.lock().unwrap();
        let connection = double.connection("conn-exa-1");
        connection.value = ROTATED_VALUE.to_owned();
        connection.rotated_at = Some("2026-09-19T12:00:00.000Z".to_owned());
    }
    let rotated = fetch(home.path(), &store);
    assert_eq!(rotated.outcome, Outcome::Accepted, "{rotated:?}");
    assert_eq!(rotated.held[0].connection_id, "conn-exa-1");
    assert_eq!(
        rotated.held[0].rotated_at.as_deref(),
        Some("2026-09-19T12:00:00.000Z")
    );
    let seen = session_handle.names();
    assert_eq!(seen[0].connection_id, "conn-exa-1");
    assert_eq!(
        seen[0].rotated_at.as_deref(),
        Some("2026-09-19T12:00:00.000Z")
    );
    assert_no_value("the operator home", &everything_under(home.path()));
}

// Catches: a second edge or another organisation's edge shown the first's
// connection; a key the hub does not hold, or has revoked, keeping a release;
// the hub's stable reason lost; an edge that knows it is revoked still asking.
#[test]
fn an_unauthorised_edge_holds_nothing_and_a_refused_one_is_cleared_with_the_reason() {
    let (home, double, handle, key_id) = authorised_edge();

    // A second edge of the same organisation, not authorised: a valid, empty
    // release. Another organisation's edge: the same, and never the id.
    for organisation in ["org-1", "org-2"] {
        let other = tempfile::tempdir().unwrap();
        enrol(other.path(), &double, &handle.url(), organisation);
        let store = store();
        let empty = fetch(other.path(), &store);
        assert_eq!(empty.outcome, Outcome::Unchanged, "{empty:?}");
        assert_eq!(empty.http_status, Some(200));
        assert!(empty.held.is_empty() && store.names().is_empty());
        assert!(!everything_under(other.path()).contains("conn-exa-1"));
    }

    // The authorised edge holds its release, then the hub stops accepting
    // its key: revoked there, which this edge has not yet learnt.
    let store = store();
    assert_eq!(fetch(home.path(), &store).held.len(), 1);
    double.lock().unwrap().keys.remove(&key_id);
    let refused = fetch(home.path(), &store);
    assert_eq!(refused.outcome, Outcome::Refused, "{refused:?}");
    assert_eq!(refused.http_status, Some(401));
    assert_eq!(refused.reason.as_deref(), Some("key_unknown"));
    assert_eq!(refused.retried_after, None);
    assert!(refused.held.is_empty() && store.names().is_empty());
    assert!(refused.journal_line().contains("refused 401: key_unknown"));

    // Once the edge has learnt of its revocation it holds no key to sign
    // with: nothing is sent, and nothing is held.
    let asked = double.lock().unwrap().received.len();
    let mut record = EnrolmentRecord::load(home.path()).unwrap().unwrap();
    record.revoked_at = Some("2026-09-19T13:00:00.000Z".to_owned());
    record.revocation = Some("owner".to_owned());
    record.store(home.path()).unwrap();
    let unsigned = fetch(home.path(), &store);
    assert_eq!(unsigned.outcome, Outcome::Refused, "{unsigned:?}");
    assert_eq!(unsigned.http_status, None);
    assert!(unsigned.reason.unwrap().contains("no request was made"));
    assert_eq!(double.lock().unwrap().received.len(), asked);
}

/// The number of requests the double has received.
fn asked(double: &Arc<Mutex<Double>>) -> usize {
    double.lock().unwrap().received.len()
}

/// The nonces of the last two requests the double received.
fn last_two_nonces(double: &Arc<Mutex<Double>>) -> (String, String) {
    let double = double.lock().unwrap();
    let nonce = |request: &Request| {
        quoted(request.headers.get("signature-input").unwrap(), "nonce").unwrap()
    };
    let [.., first, second] = double.received.as_slice() else {
        panic!("fewer than two requests");
    };
    (nonce(first), nonce(second))
}

// Catches (EGR-13): a ruling on the key signed again and asked a second time;
// a `key_revoked` or a `401` with no code (a hub before the stable codes)
// read as a fault and the set kept.
#[test]
fn a_ruling_on_the_key_clears_the_set_at_once_without_asking_again() {
    let (home, double, _handle, key_id) = authorised_edge();
    let store = store();

    let rulings = [
        (None, "key_revoked"),
        (
            Some(json!({"detail": "not signed by an enrolled key: unknown key"})),
            "not signed by an enrolled key: unknown key",
        ),
        (Some(json!({"reason": "unknown_edge"})), "unknown_edge"),
    ];
    for (forced, reason) in rulings {
        double.lock().unwrap().forced = None;
        double.lock().unwrap().revoked.clear();
        assert_eq!(fetch(home.path(), &store).held.len(), 1);
        match forced {
            Some(body) => double.lock().unwrap().forced = Some((401, body.to_string())),
            None => {
                double.lock().unwrap().revoked.insert(key_id.clone());
            }
        }
        let before = asked(&double);
        let refused = fetch(home.path(), &store);
        assert_eq!(refused.outcome, Outcome::Refused, "{refused:?}");
        assert_eq!(refused.http_status, Some(401));
        assert_eq!(refused.reason.as_deref(), Some(reason));
        assert_eq!(refused.retried_after, None);
        assert!(refused.held.is_empty() && store.names().is_empty());
        assert_eq!(asked(&double), before + 1, "{reason}: asked again");
    }
}

// Catches (EGR-13): a `401` code the edge does not know read as a fault, so a
// ruling code a later hub adds, or a ruling code padded or differently cased
// by a hub bug, keeps a set for up to three intervals. Only the seven named
// fault codes are faults; the edge fails closed on every other.
#[test]
fn a_401_code_that_is_not_a_named_fault_is_a_ruling() {
    let (home, double, _handle, _) = authorised_edge();
    let store = store();

    for code in [
        "key_suspended",
        "organisation_closed",
        " key_revoked",
        "KEY_REVOKED",
        "signature_invalid ",
        "nonce_repeated\n",
        "Nonce_Repeated",
    ] {
        assert_eq!(fetch(home.path(), &store).held.len(), 1);
        double.lock().unwrap().faults.push_back(code);
        let before = asked(&double);
        let refused = fetch(home.path(), &store);
        assert_eq!(refused.outcome, Outcome::Refused, "{code:?}: {refused:?}");
        assert_eq!(refused.http_status, Some(401));
        // The recorded reason is bounded: control characters are dropped.
        assert_eq!(refused.reason.as_deref(), Some(code.trim_end_matches('\n')));
        assert_eq!(refused.retried_after, None);
        assert!(refused.held.is_empty() && store.names().is_empty());
        assert_eq!(asked(&double), before + 1, "{code:?}: asked again");
    }
}

// Catches (EGR-13): a fault in one request (a skewed clock, a repeated nonce,
// an unverified signature) clearing the set as a ruling does; the retry
// resending the same nonce, which the hub refuses as a repeat; a second
// fault read as a ruling, or its code lost; a ruling after a fault kept.
#[test]
fn a_fault_is_signed_again_once_and_a_second_fault_keeps_the_set() {
    let (home, double, _handle, key_id) = authorised_edge();
    let store = store();
    assert_eq!(fetch(home.path(), &store).outcome, Outcome::Accepted);

    // One injected fault of each code the hub gives for one request; the
    // retry passes the double's real check.
    for code in [
        "signature_invalid",
        "signature_expired",
        "window_too_long",
        "authority_mismatch",
        "components_missing",
        "nonce_missing",
        "nonce_repeated",
    ] {
        double.lock().unwrap().faults.push_back(code);
        let before = asked(&double);
        let retried = fetch(home.path(), &store);
        assert_eq!(retried.outcome, Outcome::Unchanged, "{code}: {retried:?}");
        assert_eq!(retried.http_status, Some(200));
        assert_eq!(retried.reason, None);
        assert_eq!(retried.retried_after.as_deref(), Some(code));
        assert_eq!(retried.held.len(), 1);
        assert_eq!(asked(&double), before + 2, "{code}");
        let (first, second) = last_two_nonces(&double);
        assert_ne!(first, second, "{code}: the retry reused its nonce");
        assert_eq!(
            Fetch::read(home.path())
                .unwrap()
                .unwrap()
                .retried_after
                .as_deref(),
            Some(code)
        );
    }

    // A clock ten minutes slow: the double's own window check refuses both
    // requests, so the second fault is real, not injected.
    let before = asked(&double);
    let slow = client::fetch(
        home.path(),
        &store,
        Utc::now() - Duration::minutes(10),
        BUDGET,
    )
    .expect("recorded");
    assert_eq!(slow.outcome, Outcome::Unreachable, "{slow:?}");
    assert_eq!(slow.http_status, Some(401));
    assert_eq!(slow.reason.as_deref(), Some("signature_expired"));
    assert_eq!(slow.retried_after.as_deref(), Some("signature_expired"));
    assert_eq!(slow.held.len(), 1);
    assert_eq!(store.names()[0].connection_id, "conn-exa-1");
    assert_eq!(asked(&double), before + 2, "asked more than twice");
    assert!(
        slow.journal_line().contains(
            "unreachable 401: signature_expired (signed again after 401 signature_expired); \
             holding exa (conn-exa-1)"
        ),
        "{}",
        slow.journal_line()
    );

    // Two injected faults of different codes: the second is the one recorded.
    double
        .lock()
        .unwrap()
        .faults
        .extend(["nonce_repeated", "signature_invalid"]);
    let twice = fetch(home.path(), &store);
    assert_eq!(twice.outcome, Outcome::Unreachable, "{twice:?}");
    assert_eq!(twice.reason.as_deref(), Some("signature_invalid"));
    assert_eq!(twice.retried_after.as_deref(), Some("nonce_repeated"));
    assert_eq!(store.names().len(), 1);

    // A fault, then a ruling on the retry: the ruling stands.
    double.lock().unwrap().faults.push_back("nonce_repeated");
    double.lock().unwrap().revoked.insert(key_id);
    let ruled = fetch(home.path(), &store);
    assert_eq!(ruled.outcome, Outcome::Refused, "{ruled:?}");
    assert_eq!(ruled.reason.as_deref(), Some("key_revoked"));
    assert_eq!(ruled.retried_after.as_deref(), Some("nonce_repeated"));
    assert!(ruled.held.is_empty() && store.names().is_empty());
    assert_no_value("the operator home", &everything_under(home.path()));
}

// Catches (EGR-13): a set kept under repeated faults past its maximum age,
// which would let a hub that answers faults for ever keep a key in use.
#[test]
fn a_set_kept_under_repeated_faults_expires_at_its_maximum_age() {
    let (home, _double, _handle, _) = authorised_edge();
    let (store, at) = aged_store();
    fetch(home.path(), &store);
    let slow = || {
        client::fetch(
            home.path(),
            &store,
            Utc::now() - Duration::minutes(10),
            BUDGET,
        )
        .expect("recorded")
    };

    at(899);
    let kept = slow();
    assert_eq!(kept.outcome, Outcome::Unreachable, "{kept:?}");
    assert_eq!(kept.reason.as_deref(), Some("signature_expired"));
    assert_eq!(kept.held.len(), 1);

    at(900);
    let lapsed = slow();
    assert_eq!(lapsed.outcome, Outcome::Unreachable, "{lapsed:?}");
    assert!(lapsed.held.is_empty(), "{lapsed:?}");
    assert_eq!(lapsed.expired[0].connection_id, "conn-exa-1");
    assert_eq!(lapsed.max_age_seconds, Some(900));
    assert_exa_unavailable(&store);
}

/// The `created` parameter of the last two requests the double received.
fn last_two_created(double: &Arc<Mutex<Double>>) -> (i64, i64) {
    let double = double.lock().unwrap();
    let created = |request: &Request| {
        integer(request.headers.get("signature-input").unwrap(), "created").unwrap()
    };
    let [.., first, second] = double.received.as_slice() else {
        panic!("fewer than two requests");
    };
    (created(first), created(second))
}

// Catches (EGR-13): a retry given a fresh budget, which would stretch the
// revocation bound (the interval plus one request budget) by a second budget;
// a retry signed with the first request's `created`, which a hub counts from
// a moment already past.
#[test]
fn the_retry_is_sent_inside_the_first_requests_budget() {
    let (home, double, _handle, _) = authorised_edge();
    let store = store();
    assert_eq!(fetch(home.path(), &store).outcome, Outcome::Accepted);

    // The first answer advances the budget clock by three seconds. Only
    // the retry's answer is held: it exceeds the two seconds left, but
    // would fit a fresh five-second budget. Scheduling before the first
    // answer cannot change the retry decision.
    let budget = std::time::Duration::from_secs(5);
    let elapsed = Arc::new(std::sync::atomic::AtomicU64::new(0));
    {
        let elapsed = Arc::clone(&elapsed);
        let mut double = double.lock().unwrap();
        double.before_answer = Some(Arc::new(move || {
            if elapsed.swap(3, std::sync::atomic::Ordering::SeqCst) == 3 {
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
        }));
        double.faults.push_back("nonce_repeated");
    }
    let started = std::time::Instant::now();
    let clock = || {
        started + std::time::Duration::from_secs(elapsed.load(std::sync::atomic::Ordering::SeqCst))
    };
    let timed_out = client::fetch_with_clock(home.path(), &store, Utc::now(), budget, &clock)
        .expect("recorded");
    assert_eq!(timed_out.outcome, Outcome::Unreachable, "{timed_out:?}");
    assert_eq!(timed_out.http_status, None);
    assert!(
        timed_out
            .reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with("the hub gave no usable answer")),
        "{timed_out:?}"
    );
    assert_eq!(timed_out.retried_after.as_deref(), Some("nonce_repeated"));
    assert_eq!(asked(&double), 3, "the retry did not reach the double");
    assert_eq!(store.names().len(), 1);
    let (first, second) = last_two_created(&double);
    assert_eq!(
        second,
        first + 3,
        "the retry is signed at the advanced clock"
    );
}

// Catches (EGR-13): a retry sent with less than a quarter of the budget left,
// which can only time out and would record no status in place of the fault.
#[test]
fn a_fault_that_leaves_too_little_budget_stands_as_the_answer() {
    let (home, double, _handle, _) = authorised_edge();
    let store = store();
    assert_eq!(fetch(home.path(), &store).outcome, Outcome::Accepted);

    let elapsed = Arc::new(std::sync::atomic::AtomicU64::new(0));
    {
        let elapsed = Arc::clone(&elapsed);
        let mut double = double.lock().unwrap();
        double.before_answer = Some(Arc::new(move || {
            elapsed.store(3_200, std::sync::atomic::Ordering::SeqCst);
        }));
        double.faults.push_back("signature_expired");
    }
    let started = std::time::Instant::now();
    let clock = || {
        started
            + std::time::Duration::from_millis(elapsed.load(std::sync::atomic::Ordering::SeqCst))
    };
    let stood = client::fetch_with_clock(
        home.path(),
        &store,
        Utc::now(),
        std::time::Duration::from_secs(4),
        &clock,
    )
    .expect("recorded");
    assert_eq!(stood.outcome, Outcome::Unreachable, "{stood:?}");
    assert_eq!(stood.http_status, Some(401));
    assert_eq!(stood.reason.as_deref(), Some("signature_expired"));
    assert_eq!(stood.retried_after, None);
    assert_eq!(asked(&double), 2, "a retry was sent");
    assert_eq!(store.names().len(), 1);
}

/// A `GET` of the release route carrying the signature headers `sign` makes
/// with this home's enrolled key.
fn signed_by_hand(
    home: &Path,
    sign: impl FnOnce(&SigningIdentity, &mut Request) -> Result<(), String>,
) -> Request {
    let identity = Identity::load(home).unwrap();
    let mut request = Request::get(client::RELEASE_PATH);
    sign(identity.signer().expect("an enrolled key"), &mut request).expect("sign");
    request
}

// Catches (EGR-12): a request signed as the desired-policy request is, which
// is what a released edge and the crossing signer make, admitted at the
// release route; a signature made for another route or method admitted here;
// the release client's own signature not passing the same double.
#[test]
fn a_signature_that_is_not_bound_to_the_release_route_is_refused_there() {
    let (home, double, handle, _) = authorised_edge();
    let url = format!("{}{}", handle.url(), client::RELEASE_PATH);
    let now = Utc::now().timestamp();
    let refused_with = |request: Request| {
        let response = commonmeasure_http::send(&url, request).unwrap();
        assert_eq!(response.status, 401);
        serde_json::from_slice::<Value>(&response.body).unwrap()["code"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    // The positive control: the helper's request, signed for this route and
    // method, is admitted, so the refusals below are rulings on what each
    // signature covers and not on the helper.
    let bound = signed_by_hand(home.path(), |signer, request| {
        signer.sign_request_covering(&url, request, &[Derived::Method, Derived::Path], &[], now)
    });
    assert_eq!(commonmeasure_http::send(&url, bound).unwrap().status, 200);
    double.lock().unwrap().released.clear();

    // Signed as the desired-policy request is, under the same authority and
    // key.
    let policy_shaped = signed_by_hand(home.path(), |signer, request| {
        signer.sign_request(&url, request, &[], now)
    });
    assert_eq!(refused_with(policy_shaped), "components_missing");

    // Covers both components, for another path; then for another method.
    let other_path = signed_by_hand(home.path(), |signer, request| {
        signer.sign_request_covering(
            &format!("{}/api/v1/policy/desired", handle.url()),
            request,
            &[Derived::Method, Derived::Path],
            &[],
            now,
        )
    });
    assert_eq!(refused_with(other_path), "signature_invalid");
    let other_method = signed_by_hand(home.path(), |signer, request| {
        let mut post = Request::post(client::RELEASE_PATH, Vec::new(), "application/json");
        signer.sign_request_covering(
            &url,
            &mut post,
            &[Derived::Method, Derived::Path],
            &[],
            now,
        )?;
        for name in ["signature-agent", "signature-input", "signature"] {
            request.headers.set(name, post.headers.get(name).unwrap());
        }
        Ok(())
    });
    assert_eq!(refused_with(other_method), "signature_invalid");
    assert!(double.lock().unwrap().released.is_empty());

    // The client's own request is admitted by the same rule.
    let store = store();
    assert_eq!(fetch(home.path(), &store).outcome, Outcome::Accepted);
}

// Catches: a query or fragment in `policy_url` carried into the release
// request, which the hub refuses; the hub's `400` `query_refused` read as a
// ruling on the key and the released set cleared.
#[test]
fn a_query_never_reaches_the_release_route_and_its_refusal_keeps_the_set() {
    let (home, double, handle, _) = authorised_edge();
    let store = store();
    write_deployment(
        home.path(),
        &format!("{}/api/v1/policy/desired?tenant=a#top", handle.url()),
        "org-1",
    );
    let accepted = fetch(home.path(), &store);
    assert_eq!(accepted.outcome, Outcome::Accepted, "{accepted:?}");
    assert_eq!(
        accepted.url,
        format!("{}{}", handle.url(), client::RELEASE_PATH)
    );
    assert_eq!(
        double.lock().unwrap().received.last().unwrap().target,
        client::RELEASE_PATH
    );

    // The double refuses a query as the contract has the hub do, before it
    // looks at the signature.
    let url = format!("{}{}?tenant=a", handle.url(), client::RELEASE_PATH);
    let mut queried = Request::get(&format!("{}?tenant=a", client::RELEASE_PATH));
    Identity::load(home.path())
        .unwrap()
        .signer()
        .expect("an enrolled key")
        .sign_request_covering(
            &url,
            &mut queried,
            &[Derived::Method, Derived::Path],
            &[],
            Utc::now().timestamp(),
        )
        .unwrap();
    let refused = commonmeasure_http::send(&url, queried).unwrap();
    assert_eq!(refused.status, 400);
    let answered = serde_json::from_slice::<Value>(&refused.body).unwrap();
    assert_eq!(answered["code"], "query_refused");

    // The client cannot send a query, so the answer is forced to reach it.
    double.lock().unwrap().forced = Some((400, answered.to_string()));
    let kept = fetch(home.path(), &store);
    assert_eq!(kept.outcome, Outcome::Unreachable, "{kept:?}");
    assert_eq!(kept.http_status, Some(400));
    assert_eq!(kept.reason.as_deref(), Some("query_refused"));
    assert_eq!(kept.held.len(), 1);
    assert_eq!(store.names()[0].connection_id, "conn-exa-1");
}

// Catches: an outage or a rate limit widening or narrowing what the last
// release granted; a first fetch that fails leaving something held; the
// hub's reason or status lost; a parser message quoting a value into the
// record; a release addressed to another edge taken.
#[test]
fn an_answer_that_rules_on_nothing_keeps_the_last_release_and_grants_none() {
    let (home, double, mut handle, key_id) = authorised_edge();
    let never_fetched = store();
    let store = store();
    fetch(home.path(), &store);

    let forced = [
        (
            429,
            json!({"reason": "rate_limited"}).to_string(),
            "rate_limited",
        ),
        (
            404,
            json!({"detail": "this hub has no public origin configured"}).to_string(),
            "this hub has no public origin configured",
        ),
        (503, "upstream".to_owned(), "http_503"),
        (
            // `value` where a list belongs: a parser would quote it.
            200,
            json!({"organisation": "org-1", "key_id": key_id, "served_at": "now",
                   "credentials": ROTATED_VALUE})
            .to_string(),
            "the hub answered 200 with no release document",
        ),
        (
            200,
            json!({"organisation": "org-1", "key_id": "another-key", "served_at": "now",
                   "credentials": [{"connection_id": "conn-x", "provider": "exa",
                                    "variable": "EXA_API_KEY", "value": ROTATED_VALUE}]})
            .to_string(),
            "the hub answered 200 with a release addressed to another key or organisation",
        ),
    ];
    for (status, body, reason) in forced {
        double.lock().unwrap().forced = Some((status, body));
        for (held, store) in [(1, &store), (0, &never_fetched)] {
            let kept = fetch(home.path(), store);
            assert_eq!(kept.outcome, Outcome::Unreachable, "{status}: {kept:?}");
            assert_eq!(kept.http_status, Some(status));
            assert_eq!(kept.reason.as_deref(), Some(reason));
            assert_eq!(kept.held.len(), held, "{status}");
            assert_eq!(store.names().len(), held, "{status}");
        }
    }

    // The hub gone altogether.
    double.lock().unwrap().forced = None;
    handle.stop();
    for (held, store) in [(1, &store), (0, &never_fetched)] {
        let kept = fetch(home.path(), store);
        assert_eq!(kept.outcome, Outcome::Unreachable, "{kept:?}");
        assert_eq!(kept.http_status, None);
        assert!(kept.reason.is_some());
        assert_eq!(store.names().len(), held);
    }
    assert_eq!(store.names()[0].connection_id, "conn-exa-1");
    assert_no_value("the operator home", &everything_under(home.path()));
}

/// A peer that is not the double and not an HTTP server: it answers every
/// connection with `response` byte for byte and verifies nothing. It stands
/// for a proxy or middlebox in front of the hub that breaks the framing.
fn raw_peer(response: String) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && stream.read_exact(&mut byte).is_ok() {
                head.push(byte[0]);
            }
            let _ = stream.write_all(response.as_bytes());
        }
    });
    url
}

// Catches: the transport's error text passed into the fetch record, the
// journal line or the status answer. The HTTP client quotes a chunk-size line
// it cannot read, and a `200` declared chunked whose body is not chunk-framed
// makes the release document that line.
#[test]
fn a_misframed_answer_carrying_the_value_reaches_no_record_line_or_status() {
    let (home, double, _handle, key_id) = authorised_edge();
    let store = store();
    fetch(home.path(), &store);

    // Members in the hub's order (sorted, so `credentials` comes first),
    // compact, ending in a newline.
    let body = format!(
        "{{\"credentials\":[{{\"connection_id\":\"conn-exa-1\",\"provider\":\"exa\",\
         \"rotated_at\":null,\"value\":\"{ROTATED_VALUE}\",\"variable\":\"EXA_API_KEY\"}}],\
         \"key_id\":\"{key_id}\",\"organisation\":\"org-1\",\"served_at\":\"now\"}}\n"
    );
    let peer = raw_peer(format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
         Transfer-Encoding: chunked\r\n\r\n{body}"
    ));
    // The fault is real: the transport's own error does quote the value, so
    // the assertions below cannot pass for want of anything to leak.
    let transport = commonmeasure_http::send(
        &format!("{peer}{}", client::RELEASE_PATH),
        Request::get(client::RELEASE_PATH),
    )
    .expect_err("the body is not chunk-framed");
    assert!(format!("{transport:#}").contains(ROTATED_VALUE));

    write_deployment(
        home.path(),
        &format!("{peer}/api/v1/policy/desired"),
        "org-1",
    );
    let asked = double.lock().unwrap().received.len();
    let kept = fetch(home.path(), &store);
    assert_eq!(double.lock().unwrap().received.len(), asked);
    assert_eq!(kept.outcome, Outcome::Unreachable, "{kept:?}");
    assert_eq!(kept.http_status, None);
    // The last released set is kept, under the first value.
    assert_eq!(kept.held.len(), 1);
    assert_eq!(store.names()[0].connection_id, "conn-exa-1");

    assert_no_value(
        "the fetch, its journal line or the store",
        &format!("{kept:?} {} {store:?}", kept.journal_line()),
    );
    assert_no_value("the operator home", &everything_under(home.path()));

    std::fs::write(
        home.path().join("policy.json"),
        r#"{"policy_mode":"observe"}"#,
    )
    .unwrap();
    let mut server = McpServer::new(
        SessionLog::open(home.path(), "misframed-session").unwrap(),
        SessionPolicy::load(home.path(), None).unwrap(),
        "claude-connector",
        None,
        CredentialsStatus {
            path: home.path().join("credentials.env"),
            loaded: None,
        },
    )
    .released(store.clone());
    assert_no_value("context_status", &status_of(&mut server).to_string());
    assert!(
        kept.reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with("the hub gave no usable answer")),
        "{:?}",
        kept.reason
    );
}

// Catches: a hub setting the corpus root or the skill catalogue, which decide
// what this machine reads and runs; a variable that is not the provider's; a
// second connection for one provider held over the first.
#[test]
fn a_served_entry_the_edge_may_not_hold_is_recorded_and_not_held() {
    let (home, double, _handle, key_id) = authorised_edge();
    {
        let mut double = double.lock().unwrap();
        let entries = [
            ("conn-corpus", "internal", "COMMONMEASURE_CORPUS"),
            ("conn-skill", "skill:audit", "COMMONMEASURE_SKILLS"),
            ("conn-wrong", "tavily", "EXA_API_KEY"),
            ("conn-exa-2", "exa", "EXA_API_KEY"),
        ];
        for (id, provider, variable) in entries {
            double.connections.push(Connection {
                id,
                organisation: "org-1",
                provider,
                variable,
                value: ROTATED_VALUE.to_owned(),
                rotated_at: None,
                revoked: false,
                authorised: HashSet::from([key_id.clone()]),
            });
        }
    }
    let store = store();
    let fetched = fetch(home.path(), &store);
    assert_eq!(fetched.outcome, Outcome::Accepted, "{fetched:?}");
    assert_eq!(
        fetched
            .held
            .iter()
            .map(|held| held.connection_id.as_str())
            .collect::<Vec<_>>(),
        ["conn-exa-1"]
    );
    assert_eq!(
        fetched
            .ignored
            .iter()
            .map(|ignored| ignored.connection_id.as_str())
            .collect::<Vec<_>>(),
        ["conn-corpus", "conn-skill", "conn-wrong", "conn-exa-2"]
    );
    assert_no_value("the operator home", &everything_under(home.path()));
}

// Catches: a release request from an edge the contract excludes; a secret
// fetched over plain HTTP to another machine.
#[test]
fn an_edge_that_is_not_enrolled_managed_or_on_tls_makes_no_request() {
    let store = store();
    let bare = tempfile::tempdir().unwrap();
    let refused = client::fetch(bare.path(), &store, Utc::now(), BUDGET).unwrap_err();
    assert!(refused.contains("not enrolled"), "{refused}");

    let (home, double, _handle, _) = authorised_edge();
    std::fs::write(home.path().join("deployment.json"), r#"{"mode":"local"}"#).unwrap();
    let refused = client::fetch(home.path(), &store, Utc::now(), BUDGET).unwrap_err();
    assert!(refused.contains("deployment mode is local"), "{refused}");

    // The answer carries a secret under TLS and nothing else, so a plain
    // HTTP origin on another machine is never asked.
    write_deployment(
        home.path(),
        "http://hub.example/api/v1/policy/desired",
        "org-1",
    );
    let refused = client::fetch(home.path(), &store, Utc::now(), BUDGET).unwrap_err();
    assert!(refused.contains("https"), "{refused}");

    assert!(double.lock().unwrap().received.is_empty());
    assert!(!Fetch::path(home.path()).exists());
    assert!(store.names().is_empty());
}

fn records(home: &Path, session: &str) -> Vec<Value> {
    std::fs::read_to_string(home.join(format!("sessions/{session}.ndjson")))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn status_of(server: &mut McpServer) -> Value {
    let answer = server
        .handle_message_text(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": {"name": "context_status", "arguments": {}}})
            .to_string(),
        )
        .unwrap();
    serde_json::from_str(answer["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

/// A mediated call that writes the session's opening records and crosses
/// nothing: a private address the policy does not mediate.
fn refused_fetch(server: &mut McpServer) -> String {
    server
        .handle_message_text(
            &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                    "params": {"name": "context_fetch",
                               "arguments": {"url": "http://127.0.0.1:9/private"}}})
            .to_string(),
        )
        .unwrap()
        .to_string()
}

// Catches: a released key unnamed in the source record or in `context_status`;
// a value in the record, the status answer, a tool error or the spool; a
// session that outlives a withdrawal still recording the old connection; the
// unavailable message mentioning the hub.
#[test]
fn the_mediating_server_names_the_release_and_never_carries_the_value() {
    let (home, double, _handle, key_id) = authorised_edge();
    let store = store();
    fetch(home.path(), &store);
    std::fs::write(
        home.path().join("policy.json"),
        r#"{"policy_mode":"observe"}"#,
    )
    .unwrap();
    let mut server = McpServer::new(
        SessionLog::open(home.path(), "custody-session").unwrap(),
        SessionPolicy::load(home.path(), None).unwrap(),
        "claude-connector",
        None,
        CredentialsStatus {
            path: home.path().join("credentials.env"),
            loaded: None,
        },
    )
    .released(store.clone());
    let mut said = String::new();

    let status = status_of(&mut server);
    said.push_str(&status.to_string());
    let exa = status["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|provider| provider["provider"] == "exa")
        .unwrap()
        .clone();
    assert_eq!(exa["configured"], true);
    let member = if exa_in_environment() {
        assert_eq!(exa["source"], "environment");
        "hub_shadowed"
    } else {
        assert_eq!(exa["source"], "hub");
        "hub"
    };
    assert_eq!(
        status["credentials"][member][0]["connection_id"],
        "conn-exa-1"
    );

    said.push_str(&refused_fetch(&mut server));
    let loaded = |home: &Path| -> Vec<Value> {
        records(home, "custody-session")
            .into_iter()
            .filter(|record| record["event"] == "credentials_loaded")
            .collect()
    };
    let first = loaded(home.path());
    assert_eq!(first.len(), 1, "{first:?}");
    let credentials = &first[0]["payload"]["credentials"];
    assert_eq!(credentials["present"], false);
    assert_eq!(credentials[member][0]["connection_id"], "conn-exa-1");
    assert_eq!(credentials[member][0]["variable"], "EXA_API_KEY");

    // A refresh that changes nothing writes no second record.
    fetch(home.path(), &store);
    said.push_str(&refused_fetch(&mut server));
    assert_eq!(loaded(home.path()).len(), 1);

    // The owner withdraws; the running session sees it at the next fetch and
    // says so before its next crossing.
    double
        .lock()
        .unwrap()
        .connection("conn-exa-1")
        .authorised
        .remove(&key_id);
    fetch(home.path(), &store);
    said.push_str(&refused_fetch(&mut server));
    let after = loaded(home.path());
    assert_eq!(after.len(), 2, "{after:?}");
    assert!(after[1]["payload"]["credentials"].get("hub").is_none());

    if !exa_in_environment() {
        let answer = server
            .handle_message_text(
                &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                        "params": {"name": "context_search",
                                   "arguments": {"query": "anything", "provider": "exa"}}})
                .to_string(),
            )
            .unwrap()
            .to_string();
        assert!(
            answer.contains("unavailable: exa needs EXA_API_KEY"),
            "{answer}"
        );
        assert!(!answer.to_lowercase().contains("hub"), "{answer}");
        said.push_str(&answer);
    }

    // The relay projects and spools this home; a value must not be in what
    // it queues either. Forecast only: nothing is sent.
    let forecast = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            dry_run: true,
            receiver: Some("https://receiver.example".to_owned()),
            ..Default::default()
        },
    );
    said.push_str(&format!("{forecast:?}"));

    assert_no_value("a tool answer or the relay's report", &said);
    assert_no_value("the operator home", &everything_under(home.path()));
}

/// A store at the default interval whose clock the test moves, and the
/// function that moves it to a number of seconds after the start. An
/// injected clock: it establishes what the store does at an age, not that
/// the service's clock reaches it.
fn aged_store() -> (ReleasedStore, impl Fn(u64)) {
    let start = std::time::Instant::now();
    let elapsed = Arc::new(Mutex::new(std::time::Duration::ZERO));
    let store = ReleasedStore::with_clock(
        std::time::Duration::from_secs(300) * client::MAX_AGE_INTERVALS,
        {
            let elapsed = Arc::clone(&elapsed);
            Arc::new(move || start + *elapsed.lock().unwrap())
        },
    );
    (store, move |seconds| {
        *elapsed.lock().unwrap() = std::time::Duration::from_secs(seconds);
    })
}

fn assert_exa_unavailable(store: &ReleasedStore) {
    if exa_in_environment() {
        return;
    }
    let error = commonmeasure_supply::supplier_with_released("exa", Some(store))
        .err()
        .expect("nothing supplies the variable");
    assert_eq!(
        error,
        SupplyError::CredentialMissing {
            variable: "EXA_API_KEY".to_owned()
        }
    );
}

// Catches: an unreachable hub keeping a released key in use for ever; the
// key dropped before three intervals; an expiry the fetch record and the
// journal line do not state.
#[test]
fn an_unreachable_hub_keeps_the_last_release_for_three_intervals_and_no_longer() {
    let (home, _double, mut handle, _) = authorised_edge();
    let (store, at) = aged_store();
    fetch(home.path(), &store);
    handle.stop();

    at(899);
    let kept = fetch(home.path(), &store);
    assert_eq!(kept.outcome, Outcome::Unreachable, "{kept:?}");
    assert_eq!(kept.held.len(), 1);
    assert!(kept.expired.is_empty() && kept.max_age_seconds.is_none());
    if !exa_in_environment() {
        assert!(commonmeasure_supply::supplier_with_released("exa", Some(&store)).is_ok());
    }

    at(900);
    let lapsed = fetch(home.path(), &store);
    assert_eq!(lapsed.outcome, Outcome::Unreachable, "{lapsed:?}");
    assert!(lapsed.held.is_empty(), "{lapsed:?}");
    assert_eq!(lapsed.expired[0].connection_id, "conn-exa-1");
    assert_eq!(lapsed.max_age_seconds, Some(900));
    assert!(
        lapsed.journal_line().contains(
            "none held; no longer using exa (conn-exa-1), not confirmed by the hub for 900s"
        ),
        "{}",
        lapsed.journal_line()
    );
    assert_eq!(Fetch::read(home.path()).unwrap(), Some(lapsed));
    assert_exa_unavailable(&store);
    assert_no_value("the operator home", &everything_under(home.path()));
}

// Catches: a fetch that refuses to try (here a deployment turned local)
// leaving the released key in use for ever, since it changes nothing in the
// store and writes no record.
#[test]
fn a_fetch_that_returns_an_error_does_not_keep_the_release_past_its_age() {
    let (home, double, _handle, _) = authorised_edge();
    let (store, at) = aged_store();
    fetch(home.path(), &store);
    let recorded = Fetch::read(home.path()).unwrap();
    let asked = double.lock().unwrap().received.len();
    std::fs::write(home.path().join("deployment.json"), r#"{"mode":"local"}"#).unwrap();

    at(899);
    client::fetch(home.path(), &store, Utc::now(), BUDGET).unwrap_err();
    assert_eq!(store.names().len(), 1);

    at(900);
    let refused = client::fetch(home.path(), &store, Utc::now(), BUDGET).unwrap_err();
    assert!(refused.contains("deployment mode is local"), "{refused}");
    assert_eq!(double.lock().unwrap().received.len(), asked);
    assert_eq!(Fetch::read(home.path()).unwrap(), recorded);
    assert!(store.names().is_empty());
    assert_eq!(store.expired()[0].connection_id, "conn-exa-1");
    assert_exa_unavailable(&store);
}

// Catches: a fetch thread that has died leaving the released key in use for
// ever. Nothing fetches after the first release here, which is all a dead
// thread is to the store; the session must refuse the search by the
// variable's name and record why the release is no longer used.
#[test]
fn with_no_fetch_running_a_session_stops_using_the_release_and_records_why() {
    let (home, _double, _handle, _) = authorised_edge();
    let (store, at) = aged_store();
    fetch(home.path(), &store);
    std::fs::write(
        home.path().join("policy.json"),
        r#"{"policy_mode":"observe"}"#,
    )
    .unwrap();
    let mut server = McpServer::new(
        SessionLog::open(home.path(), "aged-session").unwrap(),
        SessionPolicy::load(home.path(), None).unwrap(),
        "claude-connector",
        None,
        CredentialsStatus {
            path: home.path().join("credentials.env"),
            loaded: None,
        },
    )
    .released(store.clone());
    let loaded = |home: &Path| -> Vec<Value> {
        records(home, "aged-session")
            .into_iter()
            .filter(|record| record["event"] == "credentials_loaded")
            .collect()
    };

    at(899);
    let mut said = refused_fetch(&mut server);
    let before = loaded(home.path());
    assert_eq!(before.len(), 1, "{before:?}");
    assert!(
        before[0]["payload"]["credentials"]
            .get("hub_expired")
            .is_none()
    );

    at(900);
    let answer = server
        .handle_message_text(
            &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                    "params": {"name": "context_search",
                               "arguments": {"query": "anything", "provider": "exa"}}})
            .to_string(),
        )
        .unwrap()
        .to_string();
    if !exa_in_environment() {
        assert!(
            answer.contains("unavailable: exa needs EXA_API_KEY"),
            "{answer}"
        );
        assert!(!answer.to_lowercase().contains("hub"), "{answer}");
    }
    said.push_str(&answer);
    said.push_str(&refused_fetch(&mut server));

    let after = loaded(home.path());
    assert_eq!(after.len(), 2, "{after:?}");
    let credentials = &after[1]["payload"]["credentials"];
    assert_eq!(credentials["hub"], json!([]));
    assert!(credentials.get("hub_shadowed").is_none());
    assert_eq!(credentials["hub_expired"][0]["connection_id"], "conn-exa-1");
    assert_eq!(credentials["hub_expired"][0]["max_age_seconds"], 900);

    let status = status_of(&mut server);
    assert_eq!(
        status["credentials"]["hub_expired"][0]["connection_id"],
        "conn-exa-1"
    );
    if !exa_in_environment() {
        let exa = status["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|provider| provider["provider"] == "exa")
            .unwrap();
        assert_eq!(exa["configured"], false);
    }
    said.push_str(&status.to_string());
    assert_no_value("a tool answer", &said);
    assert_no_value("the operator home", &everything_under(home.path()));
}

// Catches: a credential fetch made for an edge whose stored hub URL is
// cleartext. The deployment's policy URL stays on loopback, so the fetch
// would reach the double if the hub's URL were not checked.
#[test]
fn a_stored_cleartext_hub_fetches_nothing() {
    let (home, double, mut handle, _key_id) = authorised_edge();
    let port = handle
        .url()
        .rsplit(':')
        .next()
        .unwrap()
        .trim_end_matches('/')
        .to_owned();
    let mut record = EnrolmentRecord::load(home.path()).unwrap().unwrap();
    record.hub = format!("http://0.0.0.0:{port}");
    record.store(home.path()).unwrap();
    let store = store();
    let error = client::fetch(home.path(), &store, Utc::now(), BUDGET).unwrap_err();
    assert!(
        error.contains("neither https nor http to a loopback"),
        "{error}"
    );
    assert_eq!(asked(&double), 0);
    assert!(store.names().is_empty());
    handle.stop();
}
