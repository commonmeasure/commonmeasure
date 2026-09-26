//! The instance registration client and the binding it leaves, against a
//! **test double of the hub**.
//!
//! The double is a loopback `commonmeasure_http::Server`, not the hub. It
//! verifies each request by the hub's pinned rule for instance
//! registration, written here from the hub's specification and independent of the edge's signing code: the exact
//! covered components, the five-minute window with no grace, the idempotency
//! key's characters, the body digest, the nonce spent once, the Ed25519 signature over the RFC 9421
//! base under the enrolled key, and the idempotency key bound to the method,
//! absolute target and body digest. It answers the contract's status codes
//! in the shapes of the hub's handlers at hub revision `c02a081`. It
//! does not verify the policy envelope, delegations beyond their presence,
//! or anything a database decides. Its clock can be set behind or ahead of
//! this machine's, a `401` can be forced with a reason the client cannot
//! otherwise provoke, and a binding can be made to differ from the request;
//! each is a fault injected to exercise the client's handling, and
//! establishes nothing about the hub.
//!
//! What these tests establish: the bytes the client puts on the wire pass
//! that rule, the client records what the contract's answers mean, and the
//! mediating server applies the retained binding. They do not establish
//! integration with the hub; the ignored test in
//! `crates/commonmeasure-cli/tests/instance_e2e.rs` is the one that runs
//! against a real hub.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use commonmeasure_harness::enrolment::{EnrolledIdentity, EnrolledOrganization, EnrolmentRecord};
use commonmeasure_harness::identity::{EdgeKey, LifecycleRequest, RegistrationSignature};
use commonmeasure_harness::instance::{Retained, Work};
use commonmeasure_harness::mcp::McpServer;
use commonmeasure_harness::policy::SessionPolicy;
use commonmeasure_harness::session::SessionLog;
use commonmeasure_http::{Request, Response, Server, ServerHandle};
use commonmeasure_relay::instance_registration::{self as client, Evidence};
use commonmeasure_types::canonical::{canonical_json, sha256_digest};
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// The hub's interoperability vector, copied from the hub's own fixture at
/// hub revision `c4871a9`. The two files change together.
const VECTOR: &str = include_str!("fixtures/registration-request.json");

const COMPONENTS: &str = "(\"@method\" \"@target-uri\" \"content-digest\" \"idempotency-key\")";
const DELEGATION: &str = "5b0c8a52-0000-4000-8000-000000000001";
const ACCEPTANCE: &str = "5b0c8a52-0000-4000-8000-000000000002";

// Catches: the edge's signature base drifting from the vector the hub
// verifies against, the edge's signer producing another signature over it,
// and the request digest an idempotency key is bound to drifting.
#[test]
fn the_pinned_vector_is_reproduced_byte_for_byte() {
    let vector: Value = serde_json::from_str(VECTOR).unwrap();
    let text = |field: &str| vector[field].as_str().unwrap();

    // The edge loads its key from an OKP JWK; the vector's seed is the `d`.
    let seed = STANDARD.decode(text("test_seed_base64")).unwrap();
    let public = STANDARD.decode(text("public_key_base64")).unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        EdgeKey::path(home.path()),
        json!({
            "kty": "OKP", "crv": "Ed25519",
            "x": URL_SAFE_NO_PAD.encode(&public),
            "d": URL_SAFE_NO_PAD.encode(&seed),
        })
        .to_string(),
    )
    .unwrap();
    let key = EdgeKey::load(home.path()).unwrap().expect("the key loads");
    assert_eq!(key.thumbprint(), text("key_id"));

    let signed = key.sign_registration(
        text("key_id"),
        &LifecycleRequest {
            method: text("method"),
            target_uri: text("target_uri"),
            body: text("body").as_bytes(),
            idempotency_key: vector["headers"]["idempotency-key"].as_str().unwrap(),
        },
        "AAAAAAAAAAAAAAAAAAAAAA",
        vector["created"].as_i64().unwrap(),
    );
    assert_eq!(signed.base, text("signature_base"));
    let headers = &vector["headers"];
    assert_eq!(signed.content_digest, headers["content-digest"]);
    assert_eq!(signed.signature_input, headers["signature-input"]);
    // Ed25519 is deterministic, so the signer is held to the signature too.
    assert_eq!(signed.signature, headers["signature"]);
    assert_eq!(
        RegistrationSignature::request_digest(
            text("method"),
            text("target_uri"),
            &signed.content_digest
        ),
        text("request_digest")
    );

    // The fixture's own signature verifies under the fixture's key, by a
    // verifier that shares nothing with the edge's signer.
    let signature: [u8; 64] = STANDARD
        .decode(
            headers["signature"]
                .as_str()
                .unwrap()
                .strip_prefix("sig1=:")
                .and_then(|value| value.strip_suffix(':'))
                .unwrap(),
        )
        .unwrap()
        .try_into()
        .unwrap();
    VerifyingKey::from_bytes(&public.try_into().unwrap())
        .unwrap()
        .verify_strict(
            text("signature_base").as_bytes(),
            &Signature::from_bytes(&signature),
        )
        .expect("the fixture's signature verifies under the fixture's key");
}

struct Instance {
    caller: String,
    revision: i64,
    status: String,
    expires_at: DateTime<Utc>,
    accepted: Value,
}

type DuringRenew = Box<dyn FnMut(&str) + Send>;

struct Double {
    origin: String,
    signer: SigningKey,
    /// Enrolled edge keys by key id.
    keys: HashMap<String, [u8; 32]>,
    nonces: HashSet<(String, String)>,
    /// Retained operations by caller and idempotency key: the request
    /// digest, the instance and the result.
    operations: HashMap<(String, String), (String, String, Value)>,
    instances: HashMap<String, Instance>,
    reads: usize,
    requests: u64,
    /// The double's clock relative to this machine's: negative is behind.
    clock_offset: Duration,
    /// The `created` of the last request that carried one.
    last_created: Option<i64>,
    /// Answer every request `401` with this reason.
    demand: Option<&'static str>,
    /// Make the next bindings differ from the request in this member.
    misbind: Option<&'static str>,
    /// Run while a renewal is being handled, with the instance, before the
    /// answer: what another process does to the record during the send.
    during_renew: Option<DuringRenew>,
    /// The body of the last closure received.
    closed_with: Option<Value>,
}

fn refusal(status: u16, reason: &str, request: &str) -> Response {
    refusal_at(status, reason, request, Utc::now())
}

/// A refusal stamped with the double's own clock.
fn refusal_at(status: u16, reason: &str, request: &str, server_time: DateTime<Utc>) -> Response {
    Response::json(
        status,
        &json!({
            "outcome": if status == 503 { "unavailable" } else { "refused" },
            "reason": reason, "request": request, "server_time": server_time,
        })
        .to_string(),
    )
}

struct Caller {
    key_id: String,
    idempotency_key: String,
    request_digest: String,
}

impl Double {
    /// The pinned rule. Any departure is `401`.
    fn authenticate(&mut self, request: &Request) -> Result<Caller, &'static str> {
        const INVALID: &str = "invalid_registration_signature";
        if let Some(reason) = self.demand {
            return Err(reason);
        }
        if request.headers.get("authorization").is_some() {
            return Err("registration_signature_required");
        }
        let field = |name: &str| request.headers.get(name).ok_or(INVALID);
        let params = field("signature-input")?
            .strip_prefix("sig1=")
            .ok_or(INVALID)?;
        let parts: Vec<&str> = params.split(';').collect();
        if parts.len() != 7
            || parts[0] != COMPONENTS
            || parts[4] != "alg=\"ed25519\""
            || parts[6] != "tag=\"commonmeasure-registration\""
        {
            return Err(INVALID);
        }
        let number = |index: usize, prefix: &str| -> Result<i64, &'static str> {
            parts[index]
                .strip_prefix(prefix)
                .and_then(|raw| raw.parse().ok())
                .ok_or(INVALID)
        };
        let (created, expires, now) = (
            number(1, "created=")?,
            number(2, "expires=")?,
            (Utc::now() + self.clock_offset).timestamp(),
        );
        self.last_created = Some(created);
        if created > now || expires <= now || expires <= created || expires - created > 300 {
            return Err(INVALID);
        }
        let quoted = |index: usize, prefix: &str| -> Result<String, &'static str> {
            parts[index]
                .strip_prefix(prefix)
                .and_then(|raw| raw.strip_suffix('"'))
                .filter(|raw| {
                    !raw.is_empty()
                        && raw
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                })
                .map(str::to_owned)
                .ok_or(INVALID)
        };
        let key_id = quoted(3, "keyid=\"")?;
        let nonce = quoted(5, "nonce=\"")?;
        if nonce.len() < 22 {
            return Err(INVALID);
        }
        let digest = format!(
            "sha-256=:{}:",
            STANDARD.encode(Sha256::digest(&request.body))
        );
        if field("content-digest")? != digest {
            return Err(INVALID);
        }
        let idempotency_key = field("idempotency-key")?.to_owned();
        if !(1..=128).contains(&idempotency_key.len())
            || !idempotency_key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(INVALID);
        }
        let target = format!("{}{}", self.origin, request.target);
        let base = format!(
            "\"@method\": {}\n\"@target-uri\": {target}\n\"content-digest\": {digest}\n\"idempotency-key\": {idempotency_key}\n\"@signature-params\": {params}",
            request.method
        );
        let signature: [u8; 64] = field("signature")?
            .strip_prefix("sig1=:")
            .and_then(|value| value.strip_suffix(':'))
            .and_then(|value| STANDARD.decode(value).ok())
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or(INVALID)?;
        let public = self.keys.get(&key_id).ok_or("unknown_edge")?;
        VerifyingKey::from_bytes(public)
            .map_err(|_| INVALID)?
            .verify_strict(base.as_bytes(), &Signature::from_bytes(&signature))
            .map_err(|_| INVALID)?;
        if !self.nonces.insert((key_id.clone(), nonce)) {
            return Err("nonce_reused");
        }
        Ok(Caller {
            key_id,
            idempotency_key,
            request_digest: format!(
                "sha256:{:x}",
                Sha256::digest(format!("{}\n{target}\n{digest}", request.method))
            ),
        })
    }

    fn standing(&self, instance: &Instance) -> String {
        if instance.status == "active" && instance.expires_at <= Utc::now() {
            "expired".to_owned()
        } else {
            instance.status.clone()
        }
    }

    fn current(&self, id: &str, instance: &Instance) -> Value {
        // Each duty the binding's registration names, read `outstanding`:
        // the double delivers nothing onward.
        let duties: Vec<Value> = instance.accepted["binding"]["registration"]["duties"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|reference| json!({"reference": reference, "state": "outstanding"}))
            .collect();
        json!({
            "instance": id, "revision": instance.revision, "status": self.standing(instance),
            "expires_at": instance.expires_at, "ended_at": null, "reason": null,
            "accepted": instance.accepted, "duties": duties, "custody_receipts": [],
            "evidence_available": false, "online_validation_required": true,
            "next_check": Utc::now(),
        })
    }

    fn sign(&self, binding: &Value) -> Value {
        let bytes = canonical_json(binding);
        json!({
            "binding": binding, "signed_bytes": bytes, "digest": sha256_digest(bytes.as_bytes()),
            "signer": {
                "key_id": "double-registration-key", "algorithm": "ed25519",
                "public_key": STANDARD.encode(self.signer.verifying_key().to_bytes()),
                "purpose": "instance-registration",
            },
            "signature": STANDARD.encode(self.signer.sign(bytes.as_bytes()).to_bytes()),
        })
    }

    fn bind(&self, caller: &str, id: &str, revision: i64, registration: &Value) -> Value {
        let mut retained = registration.clone();
        let envelope = retained["policy"]
            .as_object_mut()
            .and_then(|policy| policy.remove("envelope"))
            .unwrap_or(Value::Null);
        let at = Utc::now();
        let mut binding = json!({
            "schema": "commonmeasure-instance-binding/v1", "purpose": "instance-registration",
            "issuer": self.origin, "organisation": "org-1", "instance": id, "revision": revision,
            "edge_key_id": caller, "operator": null, "sponsor": "research-desk",
            "delegation": registration["delegation"], "parent_chain": [],
            "valid_from": at, "expires_at": registration["expires_at"], "next_check": at,
            "online_validation_required": true, "offline_seconds": 0,
            "policy": {
                "revision": envelope["payload"]["revision"],
                "digest": envelope["payload"]["digest"],
                "applied_digest": registration["policy"]["applied_digest"],
            },
            "registration": retained,
            // Terms as the hub's binding carries them, one per reference;
            // the owner and receiver the hub resolves are not modelled.
            "duties": registration["duties"].as_array().into_iter().flatten()
                .map(|reference| json!({"id": uuid::Uuid::new_v4(), "terms": {"reference": reference}}))
                .collect::<Vec<_>>(),
        });
        match self.misbind {
            Some("organisation") => binding["organisation"] = json!("org-2"),
            Some("policy") => {
                binding["policy"]["digest"] = json!(format!("sha256:{}", "ef".repeat(32)));
            }
            Some("work") => {
                binding["registration"]["work"] = json!([
                    {"kind": "session", "namespace": "claude-code", "id": "someone-elses"}
                ]);
            }
            Some("expires_at") => binding["expires_at"] = json!(at + Duration::hours(23)),
            Some("restated_duties") => binding["registration"]["duties"] = json!([]),
            Some("duty_terms") => binding["duties"] = json!([]),
            Some(other) => panic!("no such misbinding: {other}"),
            None => {}
        }
        self.sign(&binding)
    }

    fn handle(&mut self, request: &Request) -> Response {
        self.requests += 1;
        let reference = format!("req-{}", self.requests);
        let caller = match self.authenticate(request) {
            Ok(caller) => caller,
            Err(reason) => {
                return refusal_at(401, reason, &reference, Utc::now() + self.clock_offset);
            }
        };
        let path: Vec<&str> = request.target.trim_matches('/').split('/').collect();
        let (id, action) = match (request.method.as_str(), path.as_slice()) {
            ("POST", ["api", "v1", "instances"]) => (None, "create"),
            ("POST", ["api", "v1", "instances", id, "renew"]) => (Some(*id), "renew"),
            ("POST", ["api", "v1", "instances", id, "close"]) => (Some(*id), "close"),
            ("GET", ["api", "v1", "instances", id]) => (Some(*id), "read"),
            _ => return refusal(404, "not_found", &reference),
        };
        if let Some(id) = id
            && self
                .instances
                .get(id)
                .is_none_or(|held| held.caller != caller.key_id)
        {
            return refusal(404, "not_found", &reference);
        }
        let finish = |status: u16, mut result: Value| {
            result["request"] = json!(reference);
            result["server_time"] = json!(Utc::now());
            Response::json(status, &result.to_string())
        };
        if action == "read" {
            self.reads += 1;
            let id = id.expect("a read names an instance");
            return finish(200, self.current(id, &self.instances[id]));
        }
        let operation = (caller.key_id.clone(), caller.idempotency_key.clone());
        if let Some((digest, instance, result)) = self.operations.get(&operation) {
            if *digest != caller.request_digest {
                return refusal(409, "idempotency_conflict", &reference);
            }
            let mut result = result.clone();
            result["current"] = self.current(instance, &self.instances[instance]);
            result["replayed"] = json!(true);
            return finish(200, result);
        }
        let Ok(body) = serde_json::from_slice::<Value>(&request.body) else {
            return refusal(400, "malformed_request", &reference);
        };
        let (status, instance, result) = match action {
            "create" | "renew" => {
                let registration = if action == "create" {
                    &body
                } else {
                    &body["registration"]
                };
                if registration["delegation"] != json!(DELEGATION) {
                    return refusal(503, "operator_delegation", &reference);
                }
                let Some(expires_at) = registration["expires_at"]
                    .as_str()
                    .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
                else {
                    return refusal(400, "malformed_request", &reference);
                };
                let (id, revision) = match id {
                    None => (uuid::Uuid::new_v4().to_string(), 1),
                    Some(id) => {
                        let held = &self.instances[id];
                        if self.standing(held) != "active" {
                            return refusal(409, "instance_terminal", &reference);
                        }
                        if body["expected_revision"] != json!(held.revision) {
                            return refusal(409, "revision_conflict", &reference);
                        }
                        let revision = held.revision + 1;
                        if let Some(during) = self.during_renew.as_mut() {
                            during(id);
                        }
                        (id.to_owned(), revision)
                    }
                };
                let accepted = self.bind(&caller.key_id, &id, revision, registration);
                self.instances.insert(
                    id.clone(),
                    Instance {
                        caller: caller.key_id.clone(),
                        revision,
                        status: "active".to_owned(),
                        expires_at: expires_at.with_timezone(&Utc),
                        accepted: accepted.clone(),
                    },
                );
                (
                    if action == "create" { 201 } else { 200 },
                    id.clone(),
                    json!({"instance": id, "revision": revision, "accepted": accepted}),
                )
            }
            _ => {
                let id = id.expect("a closure names an instance").to_owned();
                let held = self.instances.get_mut(&id).expect("checked above");
                if matches!(held.status.as_str(), "closed" | "revoked") {
                    return refusal(409, "instance_terminal", &reference);
                }
                if body["expected_revision"] != json!(held.revision) {
                    return refusal(409, "revision_conflict", &reference);
                }
                // As the hub does: a duty reported outstanding must be one
                // the binding it signed last restates.
                let carried = held.accepted["binding"]["registration"]["duties"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                if body["outstanding_duties"]
                    .as_array()
                    .is_some_and(|duties| duties.iter().any(|duty| !carried.contains(duty)))
                {
                    return refusal(400, "unknown_duty_reference", &reference);
                }
                held.status = "closed".to_owned();
                self.closed_with = Some(body.clone());
                let revision = held.revision;
                let receipt = self.sign(&json!({
                    "schema": "commonmeasure-instance-custody/v1", "instance": id,
                    "revision": revision, "references": body["evidence"],
                    "custody": "references_only", "evidence_available": false,
                }));
                (
                    200,
                    id.clone(),
                    json!({"instance": id, "revision": revision, "outcome": body["outcome"],
                        "closed_at": Utc::now(), "receipt": receipt}),
                )
            }
        };
        self.operations.insert(
            operation,
            (caller.request_digest, instance.clone(), result.clone()),
        );
        let mut result = result;
        result["current"] = self.current(&instance, &self.instances[&instance]);
        finish(status, result)
    }
}

/// A managed, enrolled edge in `home` whose hub is the double, and the
/// double itself.
fn edge_and_double(home: &Path) -> (Arc<Mutex<Double>>, ServerHandle) {
    let server = Server::bind("127.0.0.1:0").unwrap();
    // The double is reached on loopback and signs and verifies for another
    // public origin, as a hub behind a proxy does: the edge must sign for the
    // origin it was given at enrolment, whatever URL it sends to.
    let url = format!("http://{}", server.local_addr().unwrap());
    let origin = "https://hub.example".to_owned();

    let key = EdgeKey::generate().unwrap();
    key.store(home).unwrap();
    let key_id = key.thumbprint();
    let public: [u8; 32] = URL_SAFE_NO_PAD
        .decode(key.jwk_x())
        .unwrap()
        .try_into()
        .unwrap();
    EnrolmentRecord {
        hub: url.clone(),
        organization: EnrolledOrganization {
            id: "org-1".to_owned(),
            name: "Org".to_owned(),
        },
        name: "laptop".to_owned(),
        key_id: key_id.clone(),
        identity: EnrolledIdentity {
            origin: origin.clone(),
            bot_page: format!("{origin}/bot"),
            contact: None,
        },
        enrolled_at: "2026-09-18T00:00:00.000Z".to_owned(),
        revoked_at: None,
        revocation: None,
        revocation_learnt_at: None,
    }
    .store(home)
    .unwrap();

    // What `connect --managed` and one policy synchronisation leave. The
    // double does not verify the envelope, so its signature is a placeholder.
    let digest = format!("sha256:{}", "ab".repeat(32));
    std::fs::write(
        home.join("deployment.json"),
        json!({
            "mode": "managed", "organisation": "org-1",
            "policy_url": format!("{url}/api/v1/policy/desired"),
            "signer": {"key_id": "policy-1", "algorithm": "ed25519", "public_key": "00".repeat(32)},
        })
        .to_string(),
    )
    .unwrap();
    std::fs::create_dir_all(home.join("managed")).unwrap();
    std::fs::write(
        home.join("managed/state.json"),
        json!({"applied": {
            "revision": 4, "digest": digest, "issued_at": "2026-09-18T00:00:00Z",
            "expires_at": "2026-09-19T00:00:00Z", "activated_at": "2026-09-18T00:00:01Z",
            "signer_key_id": "policy-1",
        }})
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        home.join("managed/last-known-good.json"),
        json!({"envelope": "contextops-policy-envelope/v1",
            "payload": {"revision": 4, "digest": digest, "organisation": "org-1"},
            "signature": {"algorithm": "ed25519", "key_id": "policy-1", "value": "00"}})
        .to_string(),
    )
    .unwrap();

    let double = Arc::new(Mutex::new(Double {
        origin,
        signer: SigningKey::from_bytes(&[9u8; 32]),
        keys: HashMap::from([(key_id, public)]),
        nonces: HashSet::new(),
        operations: HashMap::new(),
        instances: HashMap::new(),
        reads: 0,
        requests: 0,
        clock_offset: Duration::zero(),
        last_created: None,
        demand: None,
        misbind: None,
        during_renew: None,
        closed_with: None,
    }));
    let state = Arc::clone(&double);
    let handle = server
        .spawn(move |request| state.lock().unwrap().handle(&request))
        .unwrap();
    (double, handle)
}

fn request(session: &str, jobs: &[&str], expires_in: Duration) -> client::Request {
    let mut work = vec![Work {
        kind: "session".to_owned(),
        namespace: "claude-code".to_owned(),
        id: session.to_owned(),
    }];
    work.extend(jobs.iter().map(|job| Work {
        kind: "job".to_owned(),
        namespace: "context-job".to_owned(),
        id: (*job).to_owned(),
    }));
    client::Request {
        delegation: DELEGATION.to_owned(),
        acceptance: ACCEPTANCE.to_owned(),
        host_name: "claude-code".to_owned(),
        host_version: "2.1".to_owned(),
        purpose: "research".to_owned(),
        work,
        session_id: Some(session.to_owned()),
        replace_session: false,
        parent: None,
        predecessor: None,
        duties: Vec::new(),
        expires_at: Utc::now() + expires_in,
    }
}

// Catches: a request the pinned rule refuses; a retry registering twice; a
// reused key accepting changed bytes; renewal or closure losing the revision;
// a refusal recorded without the hub's reason; a record others can read.
#[test]
fn create_replay_conflict_renew_and_close_are_recorded() {
    let home = tempfile::tempdir().unwrap();
    let (double, mut server) = edge_and_double(home.path());
    let asked = request("s-1", &["job-1"], Duration::hours(1));

    let created = client::register(home.path(), &asked, "create-1", Utc::now()).unwrap();
    assert_eq!(created.operation.outcome, "accepted", "{created:?}");
    assert_eq!(created.operation.http_status, Some(201));
    let held = created.retained.expect("a retained binding");
    assert_eq!((held.revision, held.status.as_str()), (1, "active"));
    assert_eq!(held.issuer, double.lock().unwrap().origin);
    assert!(held.online_validation_required);
    assert_eq!(held.registration["policy"].get("envelope"), None);
    assert_eq!(
        held.registration["policy"]["applied_digest"],
        held.accepted["binding"]["policy"]["digest"]
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&created.record)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{}", created.record.display());
    }

    // The retry keeps its key and signs a fresh nonce: the same instance.
    let again = client::register(home.path(), &asked, "create-1", Utc::now()).unwrap();
    assert_eq!(again.operation.outcome, "replayed", "{again:?}");
    assert_eq!(again.retained.unwrap().instance, held.instance);
    assert_eq!(double.lock().unwrap().instances.len(), 1);

    // Run again with the same key and another request, the command resends
    // the first attempt's bytes: a retry never becomes a conflict because
    // the second run computed a later expiry.
    let changed = request("s-1", &["job-2"], Duration::hours(1));
    let resent = client::register(home.path(), &changed, "create-1", Utc::now()).unwrap();
    assert_eq!(resent.operation.outcome, "replayed", "{resent:?}");
    assert_eq!(resent.retained.unwrap().instance, held.instance);

    // A missing dependency at the hub is unavailable, not a weaker success.
    let mut undelegated = request("s-2", &[], Duration::hours(1));
    undelegated.delegation = "5b0c8a52-0000-4000-8000-00000000dead".to_owned();
    let refused = client::register(home.path(), &undelegated, "create-2", Utc::now()).unwrap();
    assert_eq!(refused.operation.outcome, "unavailable");
    assert_eq!(
        refused.operation.reason.as_deref(),
        Some("operator_delegation")
    );
    assert!(Retained::for_session(home.path(), "s-2").unwrap().is_none());

    let renewed = client::renew(
        home.path(),
        &held.instance,
        Utc::now() + Duration::hours(2),
        "renew-1",
        Utc::now(),
    )
    .unwrap();
    assert_eq!(renewed.operation.outcome, "accepted", "{renewed:?}");
    let renewed = renewed.retained.unwrap();
    assert_eq!(renewed.revision, 2);
    assert!(renewed.expires_at > held.expires_at);
    assert_eq!(
        Retained::for_session(home.path(), "s-1")
            .unwrap()
            .unwrap()
            .reference()
            .revision,
        2
    );

    // A registration replayed after the renewal does not put revision 1 back.
    let replayed = client::register(home.path(), &asked, "create-1", Utc::now()).unwrap();
    assert_eq!(replayed.operation.outcome, "replayed");
    assert_eq!(replayed.retained.unwrap().revision, 2);

    let read = client::status(home.path(), &held.instance, Utc::now()).unwrap();
    assert_eq!(read.operation.outcome, "accepted");
    assert_eq!(read.retained.unwrap().status, "active");

    let evidence = [Evidence {
        reference: "sessions/s-1.ndjson".to_owned(),
        digest: format!("sha256:{}", "cd".repeat(32)),
    }];
    let closed = client::close(
        home.path(),
        &held.instance,
        "completed",
        &evidence,
        "close-1",
        Utc::now(),
    )
    .unwrap();
    assert_eq!(closed.operation.outcome, "accepted", "{closed:?}");
    let closed = closed.retained.unwrap();
    assert_eq!(closed.status, "closed");
    assert_eq!(closed.closure.as_ref().unwrap()["outcome"], "completed");
    assert_eq!(
        closed.closure.as_ref().unwrap()["receipt"]["binding"]["custody"],
        "references_only"
    );
    assert_eq!(
        closed
            .operations
            .iter()
            .map(|op| op.operation.as_str())
            .collect::<Vec<_>>(),
        [
            "register", "register", "register", "renew", "register", "status", "close"
        ]
    );
    // A closed instance no longer has the session's records attributed to it.
    assert!(Retained::for_session(home.path(), "s-1").unwrap().is_none());

    // With the kept attempt gone, the same key over other bytes is the
    // hub's conflict, recorded with its reason.
    std::fs::remove_file(home.path().join("instances/attempts/create-1.json")).unwrap();
    let conflict = client::register(home.path(), &changed, "create-1", Utc::now()).unwrap();
    assert_eq!(conflict.operation.outcome, "refused");
    assert_eq!(conflict.operation.http_status, Some(409));
    assert_eq!(
        conflict.operation.reason.as_deref(),
        Some("idempotency_conflict")
    );
    let attempt: Value = serde_json::from_slice(&std::fs::read(&conflict.record).unwrap()).unwrap();
    assert_eq!(attempt["last"]["reason"], "idempotency_conflict");

    let late = client::renew(
        home.path(),
        &held.instance,
        Utc::now() + Duration::hours(1),
        "renew-2",
        Utc::now(),
    )
    .unwrap();
    assert_eq!(late.operation.reason.as_deref(), Some("instance_terminal"));
    server.stop();
}

/// A registration that names a reporting duty sends it, a renewal restates
/// it, the closure reports it outstanding, and the record keeps the duty as
/// the hub last answered it. The double binds any reference and reads it
/// `outstanding`; whether a hub resolves the reference, and what it later
/// reads, is the hub's and is not established here.
#[test]
fn a_named_duty_is_sent_restated_and_reported_outstanding_at_closure() {
    let home = tempfile::tempdir().unwrap();
    let (double, mut handle) = edge_and_double(home.path());
    let duty = "owner:7b0f3c52-2f0e-4d0a-9a53-0c1d5e6f7a81";
    let mut naming = request("duty-session", &[], Duration::minutes(30));
    naming.duties = vec![duty.to_owned()];

    let registered = client::register(home.path(), &naming, "duty-create", Utc::now()).unwrap();
    assert!(registered.accepted());
    let held = registered.retained.expect("a retained record");
    assert_eq!(held.registration["duties"], json!([duty]));
    assert_eq!(
        held.duties,
        Some(json!([{"reference": duty, "state": "outstanding"}]))
    );

    let renewed = client::renew(
        home.path(),
        &held.instance,
        Utc::now() + Duration::minutes(45),
        "duty-renew",
        Utc::now(),
    )
    .unwrap();
    assert!(renewed.accepted(), "{:?}", renewed.operation);
    assert_eq!(
        double.lock().unwrap().instances[&held.instance].accepted["binding"]["registration"]["duties"],
        json!([duty])
    );

    let closed = client::close(
        home.path(),
        &held.instance,
        "completed",
        &[],
        "duty-close",
        Utc::now(),
    )
    .unwrap();
    assert!(closed.accepted(), "{:?}", closed.operation);
    assert_eq!(
        double.lock().unwrap().closed_with.as_ref().unwrap()["outstanding_duties"],
        json!([duty])
    );
    handle.stop();

    // A registration naming none closes with none, as before.
    let plain_home = tempfile::tempdir().unwrap();
    let (plain_double, mut plain_handle) = edge_and_double(plain_home.path());
    let plain = client::register(
        plain_home.path(),
        &request("plain-session", &[], Duration::minutes(30)),
        "plain-create",
        Utc::now(),
    )
    .unwrap()
    .retained
    .unwrap();
    assert_eq!(plain.duties, None);
    client::close(
        plain_home.path(),
        &plain.instance,
        "completed",
        &[],
        "plain-close",
        Utc::now(),
    )
    .unwrap();
    assert_eq!(
        plain_double.lock().unwrap().closed_with.as_ref().unwrap()["outstanding_duties"],
        json!([])
    );
    plain_handle.stop();
}

// Catches: a registration attempted by an edge with no key, or with no
// hub-issued policy to bind, reaching the hub or failing without saying what
// is missing.
#[test]
fn an_unenrolled_or_unmanaged_edge_refuses_and_names_what_is_missing() {
    let home = tempfile::tempdir().unwrap();
    let asked = request("s-1", &[], Duration::hours(1));
    let error = client::register(home.path(), &asked, "create-1", Utc::now()).unwrap_err();
    assert!(error.contains("not enrolled"), "{error}");
    assert!(error.contains("commonmeasure connect"), "{error}");

    let (double, mut server) = edge_and_double(home.path());
    std::fs::remove_file(home.path().join("deployment.json")).unwrap();
    let error = client::register(home.path(), &asked, "create-1", Utc::now()).unwrap_err();
    assert!(error.contains("not managed"), "{error}");
    assert_eq!(double.lock().unwrap().requests, 0);
    assert!(!commonmeasure_harness::instance::directory(home.path()).exists());
    server.stop();
}

/// A page origin that counts what reaches it.
fn page() -> (Arc<Mutex<usize>>, ServerHandle) {
    let hits = Arc::new(Mutex::new(0));
    let counted = Arc::clone(&hits);
    let handle = Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(move |request| {
            *counted.lock().unwrap() += 1;
            if request.target == "/doc" {
                Response::text(200, "Registered instances fetch under policy.")
            } else {
                Response::text(404, "not found")
            }
        })
        .unwrap();
    (hits, handle)
}

fn mediating_server(home: &Path, session: &str) -> McpServer {
    mediating_server_under(
        home,
        session,
        r#"{"policy_mode":"observe","allow_private_hosts":true}"#,
    )
}

/// The same, enforcing `policy` as the edge's cached source policy.
fn mediating_server_under(home: &Path, session: &str, policy: &str) -> McpServer {
    std::fs::write(home.join("policy.json"), policy).unwrap();
    let policy = SessionPolicy::load(home, None).expect("the policy loads");
    let log = SessionLog::open(home, session).expect("session log");
    let credentials = commonmeasure_supply::credentials::CredentialsStatus {
        path: home.join(commonmeasure_supply::credentials::CREDENTIALS_FILE),
        loaded: None,
    };
    McpServer::new(log, policy, "claude-code", None, credentials)
}

/// Call `context_fetch` and answer whether the tool reported an error, with
/// its text.
fn fetch(server: &mut McpServer, url: &str) -> (bool, String) {
    let answer = server
        .handle_message_text(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {"name": "context_fetch", "arguments": {"url": url}}})
            .to_string(),
        )
        .expect("an answer");
    (
        answer["result"]["isError"] == json!(true),
        answer["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    )
}

fn records(home: &Path, session: &str) -> Vec<Value> {
    SessionLog::read(&home.join("sessions").join(format!("{session}.ndjson"))).unwrap_or_default()
}

// Catches: a registered session's records missing the reference or carrying
// a stale revision; a fetch going ahead without the online check; a session
// with no registration gaining a member, a check or a refusal.
#[test]
fn a_registered_session_is_checked_and_stamped_and_an_unregistered_one_is_unchanged() {
    let home = tempfile::tempdir().unwrap();
    let (double, mut hub) = edge_and_double(home.path());
    let (hits, mut origin) = page();
    let url = format!("{}/doc", origin.url().trim_end_matches('/'));

    let mut plain = mediating_server(home.path(), "plain");
    let (is_error, text) = fetch(&mut plain, &url);
    assert!(!is_error, "{text}");
    assert_eq!(
        double.lock().unwrap().requests,
        0,
        "no registration, no hub request"
    );
    let unregistered = records(home.path(), "plain");
    assert!(!unregistered.is_empty());
    assert!(
        unregistered
            .iter()
            .all(|record| record["payload"].get("instance").is_none())
    );

    let asked = request("s-1", &[], Duration::hours(1));
    let held = client::register(home.path(), &asked, "create-1", Utc::now())
        .unwrap()
        .retained
        .unwrap();
    let mut registered = mediating_server(home.path(), "s-1");
    let (is_error, text) = fetch(&mut registered, &url);
    assert!(!is_error, "{text}");
    assert_eq!(
        double.lock().unwrap().reads,
        1,
        "one standing check before the crossing"
    );

    client::renew(
        home.path(),
        &held.instance,
        Utc::now() + Duration::hours(2),
        "renew-1",
        Utc::now(),
    )
    .unwrap();
    let (is_error, text) = fetch(&mut registered, &url);
    assert!(!is_error, "{text}");

    let stamped = records(home.path(), "s-1");
    let revisions: Vec<i64> = stamped
        .iter()
        .filter(|record| record["event"] == "crossing_mediated")
        .map(|record| {
            let instance = &record["payload"]["instance"];
            assert_eq!(instance["issuer"], json!(held.issuer));
            assert_eq!(instance["id"], json!(held.instance));
            instance["revision"].as_i64().unwrap()
        })
        .collect();
    assert_eq!(revisions, [1, 2]);
    assert!(
        stamped
            .iter()
            .all(|record| record["payload"].get("instance").is_some())
    );
    assert!(*hits.lock().unwrap() > 0);
    hub.stop();
    origin.stop();
}

// Catches: a fetch crossing after the validity window passed; a fetch
// crossing when the required check cannot be made; either refusal missing
// from the record or recorded as the other; the hub asked about a window the
// edge already knows has passed.
#[test]
fn an_expired_window_refuses_and_an_unreachable_hub_is_unavailable() {
    let home = tempfile::tempdir().unwrap();
    let (double, mut hub) = edge_and_double(home.path());
    let (hits, mut origin) = page();
    let url = format!("{}/doc", origin.url().trim_end_matches('/'));

    // Two seconds, so a registration slowed by a loaded machine still lands
    // inside the window; the sleep then outlasts it.
    let brief = request("expiring", &[], Duration::seconds(2));
    client::register(home.path(), &brief, "create-1", Utc::now()).unwrap();
    let lasting = request("cut-off", &[], Duration::hours(1));
    client::register(home.path(), &lasting, "create-2", Utc::now()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2100));

    let mut expiring = mediating_server(home.path(), "expiring");
    let (is_error, text) = fetch(&mut expiring, &url);
    assert!(is_error, "{text}");
    assert!(text.contains("refused before the crossing"), "{text}");
    assert!(text.contains("instance_expired"), "{text}");
    assert_eq!(
        double.lock().unwrap().reads,
        0,
        "a passed window needs no check"
    );

    // The hub stops answering: the check the binding requires cannot be made.
    hub.stop();
    let mut cut_off = mediating_server(home.path(), "cut-off");
    let (is_error, text) = fetch(&mut cut_off, &url);
    assert!(is_error, "{text}");
    assert!(text.contains("unavailable before the crossing"), "{text}");
    assert!(text.contains("instance_check_unavailable"), "{text}");

    assert_eq!(
        *hits.lock().unwrap(),
        0,
        "nothing reached the page's origin"
    );
    for (session, outcome, reason) in [
        ("expiring", "refused", "instance_expired"),
        ("cut-off", "unavailable", "instance_check_unavailable"),
    ] {
        let refusals: Vec<Value> = records(home.path(), session)
            .into_iter()
            .filter(|record| record["event"] == "instance_refused")
            .collect();
        assert_eq!(refusals.len(), 1, "{session}");
        let payload = &refusals[0]["payload"];
        assert_eq!(payload["outcome"], outcome);
        assert!(
            payload["reason"].as_str().unwrap().starts_with(reason),
            "{payload}"
        );
        assert_eq!(payload["tool"], "context_fetch");
        assert_eq!(payload["url"], json!(url));
        assert_eq!(payload["instance"]["revision"], 1);
        assert!(
            records(home.path(), session)
                .iter()
                .all(|record| record["event"] != "crossing_mediated")
        );
    }
    let held = Retained::for_session(home.path(), "cut-off")
        .unwrap()
        .unwrap();
    assert_eq!(held.operations.last().unwrap().outcome, "unreachable");
    origin.stop();
}

// Expired authority and a stale source policy are separate facts. The
// instance's window has passed, so its session's work stops; the source
// policy still rules in that session and in one with no registration.
// Catches: an expired instance relaxing or replacing the policy's ruling,
// and expiry stopping a session that holds no registration.
//
// Staleness is reported and changes no ruling. The test dates the applied
// envelope's `expires_at` in the past and asserts that `stale_since` reports
// it, but no enforcement path consults `stale_since`, and the policy enforced
// is the `policy.json` the test writes. Apart from that one assertion the
// test passes identically without the stale edit, so it does not show a stale
// policy being enforced because it is stale. The hub is the double and the
// pages are loopback origins; the hub is never asked.
#[test]
fn an_expired_instance_stops_its_session_while_a_stale_policy_still_rules() {
    let home = tempfile::tempdir().unwrap();
    let (double, mut hub) = edge_and_double(home.path());
    let (hits, mut origin) = page();
    let port = origin.addr().port();
    let admitted = format!("http://127.0.0.1:{port}/doc");
    let denied = format!("http://localhost:{port}/doc");
    let policy = r#"{"policy_mode":"strict","allow_private_hosts":true,
        "constraints":[{"kind":"denied_source_host","host":"localhost"}]}"#;

    let mut state: Value =
        serde_json::from_slice(&std::fs::read(home.path().join("managed/state.json")).unwrap())
            .unwrap();
    state["applied"]["expires_at"] = json!("2026-09-01T00:00:00Z");
    std::fs::write(home.path().join("managed/state.json"), state.to_string()).unwrap();
    let applied = commonmeasure_harness::managed::State::read(home.path())
        .unwrap()
        .applied
        .expect("an applied envelope");
    assert_eq!(
        applied.stale_since(Utc::now()).as_deref(),
        Some("2026-09-01T00:00:00Z"),
        "the cached policy is stale"
    );

    // Two seconds, as in the test above.
    let brief = request("expiring", &[], Duration::seconds(2));
    client::register(home.path(), &brief, "create-1", Utc::now()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2100));

    // The registered session: the policy rules first, then the window.
    let mut expiring = mediating_server_under(home.path(), "expiring", policy);
    let (is_error, text) = fetch(&mut expiring, &denied);
    assert!(is_error, "{text}");
    assert!(!text.contains("instance_expired"), "{text}");
    let (is_error, text) = fetch(&mut expiring, &admitted);
    assert!(is_error, "{text}");
    assert!(text.contains("instance_expired"), "{text}");
    assert_eq!(*hits.lock().unwrap(), 0, "neither fetch crossed");

    // A session with no registration under the same stale policy.
    let mut plain = mediating_server_under(home.path(), "plain", policy);
    let (is_error, text) = fetch(&mut plain, &denied);
    assert!(is_error, "{text}");
    let (is_error, text) = fetch(&mut plain, &admitted);
    assert!(!is_error, "{text}");
    // The count includes the declaration probes that precede a fetch.
    assert!(*hits.lock().unwrap() > 0, "the admitted page was fetched");

    let events = |session: &str| -> Vec<String> {
        records(home.path(), session)
            .iter()
            .filter_map(|record| record["event"].as_str())
            .filter(|event| event.starts_with("crossing_") || *event == "instance_refused")
            .map(str::to_owned)
            .collect()
    };
    assert_eq!(events("expiring"), ["crossing_refused", "instance_refused"]);
    assert_eq!(events("plain"), ["crossing_refused", "crossing_mediated"]);
    let stopped = &refusals(home.path(), "expiring")[0];
    assert_eq!(stopped["outcome"], "refused");
    assert!(
        stopped["reason"]
            .as_str()
            .unwrap()
            .starts_with("instance_expired"),
        "{stopped}"
    );
    assert_eq!(
        double.lock().unwrap().reads,
        0,
        "a passed window needs no check"
    );
    hub.stop();
    origin.stop();
}

fn refusals(home: &Path, session: &str) -> Vec<Value> {
    records(home, session)
        .into_iter()
        .filter(|record| record["event"] == "instance_refused")
        .map(|record| record["payload"].clone())
        .collect()
}

// Catches: `created` dated at the edge's own clock, which a hub a second
// behind refuses; a signature the hub did not accept recorded as `refused`,
// which the contract reserves for authority known to have ended; a ruling on
// the key recorded as `unavailable`. The clock is injected on both sides:
// the double's offset, and the `now` the client is given.
#[test]
fn a_clock_ahead_of_the_hubs_is_absorbed_and_an_unaccepted_signature_is_unavailable() {
    let home = tempfile::tempdir().unwrap();
    let (double, mut hub) = edge_and_double(home.path());
    let (hits, mut origin) = page();
    let url = format!("{}/doc", origin.url().trim_end_matches('/'));

    // This edge's clock is 20 seconds ahead of the hub's.
    double.lock().unwrap().clock_offset = Duration::seconds(-20);
    let asked = request("ahead", &[], Duration::hours(1));
    let now = Utc::now();
    let created = client::register(home.path(), &asked, "create-1", now).unwrap();
    assert_eq!(created.operation.outcome, "accepted", "{created:?}");
    assert_eq!(
        double.lock().unwrap().last_created,
        Some(now.timestamp() - 30),
        "created is dated 30 seconds before the edge's clock"
    );
    let instance = created.retained.unwrap().instance;
    let mut server = mediating_server(home.path(), "ahead");
    let (is_error, text) = fetch(&mut server, &url);
    assert!(!is_error, "{text}");
    let crossed = *hits.lock().unwrap();
    assert!(crossed > 0);

    // A minute ahead is past the allowance: the hub does not accept the
    // signature. Nothing about the instance has been ruled on.
    double.lock().unwrap().clock_offset = Duration::seconds(-60);
    let read = client::status(home.path(), &instance, Utc::now()).unwrap();
    assert_eq!(read.operation.outcome, "unavailable", "{read:?}");
    assert_eq!(read.operation.http_status, Some(401));
    assert_eq!(
        read.operation.reason.as_deref(),
        Some("invalid_registration_signature")
    );
    assert!(read.operation.hub_time.is_some(), "{read:?}");
    assert_eq!(read.retained.unwrap().status, "active");

    let (is_error, text) = fetch(&mut server, &url);
    assert!(is_error, "{text}");
    assert!(text.contains("unavailable before the crossing"), "{text}");
    assert!(text.contains("invalid_registration_signature"), "{text}");
    assert!(text.contains("the hub's clock read"), "{text}");

    // A second credential beside the signature is the same kind of answer.
    {
        let mut double = double.lock().unwrap();
        double.clock_offset = Duration::zero();
        double.demand = Some("registration_signature_required");
    }
    let (is_error, text) = fetch(&mut server, &url);
    assert!(is_error, "{text}");
    assert!(text.contains("unavailable before the crossing"), "{text}");

    // A ruling on the key is a refusal, as before.
    double.lock().unwrap().demand = Some("edge_inactive");
    let (is_error, text) = fetch(&mut server, &url);
    assert!(is_error, "{text}");
    assert!(text.contains("refused before the crossing"), "{text}");

    let recorded: Vec<(String, String)> = refusals(home.path(), "ahead")
        .iter()
        .map(|payload| {
            (
                payload["outcome"].as_str().unwrap().to_owned(),
                payload["reason"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(recorded.len(), 3, "{recorded:?}");
    assert_eq!(recorded[0].0, "unavailable");
    assert!(
        recorded[0].1.starts_with(
            "instance_check_unavailable: the hub answered 401 (invalid_registration_signature)"
        ),
        "{recorded:?}"
    );
    assert_eq!(recorded[1].0, "unavailable");
    assert!(
        recorded[1].1.contains("registration_signature_required"),
        "{recorded:?}"
    );
    assert_eq!(
        recorded[2],
        ("refused".to_owned(), "edge_inactive".to_owned())
    );
    let held = Retained::read(home.path(), &instance).unwrap().unwrap();
    assert_eq!(held.status, "active", "no answer said the authority ended");
    let outcomes: Vec<&str> = held
        .operations
        .iter()
        .filter(|operation| operation.operation == "check")
        .map(|operation| operation.outcome.as_str())
        .collect();
    assert_eq!(
        outcomes,
        ["accepted", "unavailable", "unavailable", "refused"]
    );
    assert_eq!(*hits.lock().unwrap(), crossed, "no stopped fetch crossed");
    hub.stop();
    origin.stop();
}

// Catches: a key the hub's rule refuses kept and sent, and its
// `401 invalid_registration_signature` read as a signing fault.
#[test]
fn a_key_the_hub_would_refuse_is_refused_before_anything_is_kept_or_sent() {
    let home = tempfile::tempdir().unwrap();
    let (double, mut hub) = edge_and_double(home.path());
    let asked = request("s-1", &[], Duration::hours(1));
    for key in ["my.key", "", &"k".repeat(129), "clé"] {
        let error = client::register(home.path(), &asked, key, Utc::now()).unwrap_err();
        assert!(error.contains("1 to 128 ASCII letters, digits"), "{error}");
        assert!(error.contains("Nothing was sent"), "{error}");
    }
    assert!(client::idempotency_key_ok(&"k".repeat(128)));
    assert!(client::idempotency_key_ok(&client::fresh_key("register")));
    assert_eq!(double.lock().unwrap().requests, 0);
    assert!(!home.path().join("instances/attempts").exists());
    hub.stop();
}

// Catches: a binding that names another organisation, policy, work or a
// later expiry than was asked for, or that drops a duty the request named,
// retained as this edge's authority; an
// instance the hub made and the edge rejected left with no record, so that
// `close` and `status` cannot name it; a closure that reports a duty the
// hub's binding dropped, which the hub refuses (EGR-07).
#[test]
fn a_binding_that_differs_from_the_request_is_rejected_and_the_instance_can_be_closed() {
    let home = tempfile::tempdir().unwrap();
    let (double, mut hub) = edge_and_double(home.path());

    let mut rejected = Vec::new();
    for (member, names) in [
        ("organisation", "organisation"),
        ("policy", "policy digest"),
        ("work", "work"),
        ("expires_at", "expires at"),
        ("restated_duties", "named duty"),
        ("duty_terms", "named duty"),
    ] {
        double.lock().unwrap().misbind = Some(member);
        let session = format!("s-{member}");
        let mut asked = request(&session, &[], Duration::hours(1));
        asked.duties = vec!["owner:7b0f3c52-2f0e-4d0a-9a53-0c1d5e6f7a81".to_owned()];
        let key = format!("create-{member}");
        let report = client::register(home.path(), &asked, &key, Utc::now()).unwrap();
        assert_eq!(report.operation.outcome, "binding_rejected", "{report:?}");
        let reason = report.operation.reason.clone().unwrap();
        assert!(reason.contains(names), "{member}: {reason}");
        let held = report.retained.expect("the instance is recorded");
        assert_eq!(held.status, "binding_rejected");
        assert_eq!((held.revision, held.session_id.as_deref()), (1, None));
        assert!(
            Retained::for_session(home.path(), &session)
                .unwrap()
                .is_none(),
            "nothing is attributed to a rejected binding"
        );
        rejected.push(held.instance);
    }
    double.lock().unwrap().misbind = None;

    // The hub says the instance is active; the edge still has not accepted
    // its binding.
    let read = client::status(home.path(), &rejected[0], Utc::now()).unwrap();
    assert_eq!(read.operation.outcome, "accepted");
    assert_eq!(read.retained.unwrap().status, "binding_rejected");
    let closed = client::close(
        home.path(),
        &rejected[0],
        "failed",
        &[],
        "close-rejected",
        Utc::now(),
    )
    .unwrap();
    assert_eq!(closed.operation.outcome, "accepted", "{closed:?}");
    assert_eq!(closed.retained.unwrap().status, "closed");
    assert_eq!(
        double.lock().unwrap().instances[&rejected[0]].status,
        "closed"
    );

    // The hub dropped the named duty from the binding it signed, so the
    // closure reports none outstanding; naming it would be refused
    // `unknown_duty_reference` and the instance would stay active (EGR-07).
    let closed = client::close(
        home.path(),
        &rejected[4],
        "failed",
        &[],
        "close-restated-duties",
        Utc::now(),
    )
    .unwrap();
    assert_eq!(closed.operation.outcome, "accepted", "{closed:?}");
    assert_eq!(
        double.lock().unwrap().closed_with.as_ref().unwrap()["outstanding_duties"],
        json!([])
    );
    // Terms dropped with the restatement intact: the hub still holds the
    // duty, and the closure reports it.
    let closed = client::close(
        home.path(),
        &rejected[5],
        "failed",
        &[],
        "close-duty-terms",
        Utc::now(),
    )
    .unwrap();
    assert_eq!(closed.operation.outcome, "accepted", "{closed:?}");
    assert_eq!(
        double.lock().unwrap().closed_with.as_ref().unwrap()["outstanding_duties"],
        json!(["owner:7b0f3c52-2f0e-4d0a-9a53-0c1d5e6f7a81"])
    );

    // A renewal whose binding is rejected: the hub is at revision 2, the
    // session's crossings stop, and the instance still closes.
    let asked = request("renewing", &[], Duration::hours(1));
    let held = client::register(home.path(), &asked, "create-renewing", Utc::now())
        .unwrap()
        .retained
        .unwrap();
    assert_eq!(held.status, "active");
    double.lock().unwrap().misbind = Some("work");
    let renewed = client::renew(
        home.path(),
        &held.instance,
        Utc::now() + Duration::hours(2),
        "renew-rejected",
        Utc::now(),
    )
    .unwrap();
    assert_eq!(renewed.operation.outcome, "binding_rejected", "{renewed:?}");
    let after = renewed.retained.unwrap();
    assert_eq!(
        (after.revision, after.status.as_str()),
        (2, "binding_rejected")
    );
    assert_eq!(
        after.accepted["binding"]["revision"], 1,
        "the binding kept is the one that verified"
    );
    let reads = double.lock().unwrap().reads;
    let mut server = mediating_server(home.path(), "renewing");
    let (is_error, text) = fetch(&mut server, "http://127.0.0.1:9/doc");
    assert!(is_error, "{text}");
    assert!(text.contains("instance_binding_rejected"), "{text}");
    assert_eq!(double.lock().unwrap().reads, reads, "the hub is not asked");
    let closed = client::close(
        home.path(),
        &held.instance,
        "failed",
        &[],
        "close-renewing",
        Utc::now(),
    )
    .unwrap();
    assert_eq!(closed.operation.outcome, "accepted", "{closed:?}");

    // A renewal whose binding drops the duty the first binding carried: the
    // record keeps the binding that verified, and the closure follows the
    // one the hub signed last.
    double.lock().unwrap().misbind = None;
    let mut asked = request("renewing-duty", &[], Duration::hours(1));
    asked.duties = vec!["owner:7b0f3c52-2f0e-4d0a-9a53-0c1d5e6f7a81".to_owned()];
    let held = client::register(home.path(), &asked, "create-renewing-duty", Utc::now())
        .unwrap()
        .retained
        .unwrap();
    assert_eq!(held.status, "active");
    double.lock().unwrap().misbind = Some("restated_duties");
    let renewed = client::renew(
        home.path(),
        &held.instance,
        Utc::now() + Duration::hours(2),
        "renew-dropped-duty",
        Utc::now(),
    )
    .unwrap();
    assert_eq!(renewed.operation.outcome, "binding_rejected", "{renewed:?}");
    let closed = client::close(
        home.path(),
        &held.instance,
        "failed",
        &[],
        "close-renewing-duty",
        Utc::now(),
    )
    .unwrap();
    assert_eq!(closed.operation.outcome, "accepted", "{closed:?}");
    assert_eq!(
        double.lock().unwrap().closed_with.as_ref().unwrap()["outstanding_duties"],
        json!([])
    );
    hub.stop();
}

// Catches: a kept attempt, which names one hub's delegation and carries its
// policy envelope, resent to the hub the edge enrolled with afterwards.
#[test]
fn a_kept_attempt_is_not_resent_to_another_hub() {
    let home = tempfile::tempdir().unwrap();
    let (double, mut hub) = edge_and_double(home.path());
    let asked = request("s-1", &[], Duration::hours(1));
    client::register(home.path(), &asked, "create-1", Utc::now()).unwrap();
    let first = double.lock().unwrap().origin.clone();
    let sent = double.lock().unwrap().requests;

    let mut record = EnrolmentRecord::load(home.path()).unwrap().unwrap();
    let first_hub = record.hub.clone();
    record.hub = "http://127.0.0.1:9".to_owned();
    record.store(home.path()).unwrap();
    let error = client::register(home.path(), &asked, "create-1", Utc::now()).unwrap_err();
    assert!(error.contains(&first_hub), "{error} (origin {first})");
    assert!(error.contains("http://127.0.0.1:9"), "{error}");
    assert!(error.contains("choose another key"), "{error}");
    assert_eq!(double.lock().unwrap().requests, sent);
    hub.stop();
}

// Catches: the refusal quoting a kept attempt's hub URL, which an attempt
// made under 0.4.1 holds as that release's `connect` stored it, with any
// credentials in it. Both hubs are named by origin alone.
#[test]
fn a_kept_attempt_for_another_hub_names_both_by_origin_alone() {
    let home = tempfile::tempdir().unwrap();
    let (double, mut hub) = edge_and_double(home.path());
    let asked = request("s-1", &[], Duration::hours(1));
    client::register(home.path(), &asked, "create-1", Utc::now()).unwrap();
    let sent = double.lock().unwrap().requests;
    let first_hub = EnrolmentRecord::load(home.path()).unwrap().unwrap().hub;
    let planted = first_hub.replace("http://", "http://ops:ak_PLANTED@");
    let attempt = commonmeasure_harness::instance::directory(home.path())
        .join("attempts")
        .join("create-1.json");
    let kept = std::fs::read_to_string(&attempt).unwrap();
    assert!(kept.contains(&first_hub), "{kept}");
    std::fs::write(&attempt, kept.replace(&first_hub, &planted)).unwrap();

    // The same origin: another URL of it.
    let error = client::register(home.path(), &asked, "create-1", Utc::now()).unwrap_err();
    assert!(
        error.contains(&format!(
            "was used for a request to another URL of the hub at {first_hub}, and this edge is \
             now enrolled with the hub at {first_hub}; choose another key"
        )),
        "{error}"
    );
    assert!(!error.contains("ak_PLANTED"), "{error}");

    let mut record = EnrolmentRecord::load(home.path()).unwrap().unwrap();
    record.hub = "http://127.0.0.1:9".to_owned();
    record.store(home.path()).unwrap();
    let error = client::register(home.path(), &asked, "create-1", Utc::now()).unwrap_err();
    assert!(
        error.contains(&format!(
            "was used for a request to the hub at {first_hub}, and this edge is now enrolled \
             with the hub at http://127.0.0.1:9; choose another key"
        )),
        "{error}"
    );
    assert!(!error.contains("ak_PLANTED"), "{error}");
    assert_eq!(double.lock().unwrap().requests, sent);
    hub.stop();
}

// Catches: `disconnect` leaving sessions pointed at instances the next key
// cannot read, so that every crossing is refused `not_found`; the records
// going with the pointers.
#[test]
fn disconnect_retires_the_session_pointers_and_keeps_the_records() {
    let home = tempfile::tempdir().unwrap();
    let (_double, mut hub) = edge_and_double(home.path());
    let held = client::register(
        home.path(),
        &request("s-1", &[], Duration::hours(1)),
        "create-1",
        Utc::now(),
    )
    .unwrap()
    .retained
    .unwrap();
    assert!(Retained::for_session(home.path(), "s-1").unwrap().is_some());

    // No ingest key is configured here, so the hub is not asked to revoke;
    // the local half of `disconnect` is what is under test.
    let report = commonmeasure_relay::disconnect(home.path()).unwrap();
    assert!(report.revoked_at_hub.is_err());
    assert_eq!(report.retired_instance_sessions, Ok(vec!["s-1".to_owned()]));
    assert!(Retained::for_session(home.path(), "s-1").unwrap().is_none());
    assert!(
        Retained::read(home.path(), &held.instance)
            .unwrap()
            .is_some()
    );
    hub.stop();
}

// Catches: a second registration for a session silently moving its pointer
// while the first instance stays active at the hub; the refusal leaving an
// attempt file or reaching the hub; the replacement not naming what it
// replaced; a retry of the first key moving the pointer back unasked.
#[test]
fn a_session_pointed_at_an_active_instance_is_not_registered_again_without_the_flag() {
    let home = tempfile::tempdir().unwrap();
    let (double, mut hub) = edge_and_double(home.path());
    let asked = request("s-1", &[], Duration::hours(1));
    let first = client::register(home.path(), &asked, "create-1", Utc::now())
        .unwrap()
        .retained
        .unwrap();
    let sent = double.lock().unwrap().requests;

    let error = client::register(home.path(), &asked, "create-2", Utc::now()).unwrap_err();
    assert!(error.contains(&first.instance), "{error}");
    assert!(error.contains("--replace-session"), "{error}");
    assert_eq!(double.lock().unwrap().requests, sent);
    assert!(
        !home
            .path()
            .join("instances/attempts/create-2.json")
            .exists()
    );

    let mut replacing = asked.clone();
    replacing.replace_session = true;
    let second = client::register(home.path(), &replacing, "create-2", Utc::now())
        .unwrap()
        .retained
        .unwrap();
    assert_ne!(second.instance, first.instance);
    assert_eq!(
        second.session_taken_from.as_deref(),
        Some(first.instance.as_str())
    );
    assert_eq!(
        Retained::for_session(home.path(), "s-1")
            .unwrap()
            .unwrap()
            .instance,
        second.instance
    );
    // The first instance is untouched and still closes.
    assert_eq!(
        Retained::read(home.path(), &first.instance)
            .unwrap()
            .unwrap()
            .status,
        "active"
    );

    // A retry of the second key is for the instance the session points at.
    let again = client::register(home.path(), &asked, "create-2", Utc::now()).unwrap();
    assert_eq!(again.operation.outcome, "replayed", "{again:?}");
    assert_eq!(
        again.retained.unwrap().session_taken_from.as_deref(),
        Some(first.instance.as_str())
    );
    // A retry of the first key would move the pointer back.
    let error = client::register(home.path(), &asked, "create-1", Utc::now()).unwrap_err();
    assert!(error.contains(&second.instance), "{error}");
    hub.stop();
}

// Catches: a renewal writing back the record it read before the send, and
// dropping what the mediating server recorded while the hub was answering.
#[test]
fn an_operation_recorded_during_a_send_is_kept() {
    let home = tempfile::tempdir().unwrap();
    let (double, mut hub) = edge_and_double(home.path());
    let held = client::register(
        home.path(),
        &request("s-1", &[], Duration::hours(1)),
        "create-1",
        Utc::now(),
    )
    .unwrap()
    .retained
    .unwrap();

    let path = home.path().to_path_buf();
    double.lock().unwrap().during_renew = Some(Box::new(move |instance| {
        Retained::update(&path, instance, |held| {
            let mut check = held.operations[0].clone();
            check.operation = "check".to_owned();
            check.idempotency_key = "check-during-renew".to_owned();
            held.operations.push(check);
        })
        .unwrap();
    }));
    let renewed = client::renew(
        home.path(),
        &held.instance,
        Utc::now() + Duration::hours(2),
        "renew-1",
        Utc::now(),
    )
    .unwrap()
    .retained
    .unwrap();
    assert_eq!(renewed.revision, 2);
    assert_eq!(
        renewed
            .operations
            .iter()
            .map(|operation| operation.operation.as_str())
            .collect::<Vec<_>>(),
        ["register", "check", "renew"]
    );
    hub.stop();
}

/// Point the enrolment and an instance record at the double through
/// `http://0.0.0.0:<port>`: a cleartext hub URL that still reaches the
/// double, so a request a guard failed to stop would be counted.
fn store_cleartext_hub(home: &Path, hub: &ServerHandle, instance: &str) -> String {
    let port = hub
        .url()
        .rsplit(':')
        .next()
        .unwrap()
        .trim_end_matches('/')
        .to_owned();
    let cleartext = format!("http://0.0.0.0:{port}");
    let mut record = EnrolmentRecord::load(home).unwrap().unwrap();
    record.hub = cleartext.clone();
    record.store(home).unwrap();
    Retained::update(home, instance, |held| {
        held.hub = cleartext.clone();
        held.online_validation_required = true;
    })
    .unwrap();
    cleartext
}

// Catches: a register, renew, close or status request, or the check before
// a mediated crossing, sent to a stored cleartext hub; a register that keeps
// an attempt for a request it refused to send.
#[test]
fn a_stored_cleartext_hub_is_sent_no_instance_request() {
    let home = tempfile::tempdir().unwrap();
    let (double, mut hub) = edge_and_double(home.path());
    let (hits, mut origin) = page();
    let url = format!("{}/doc", origin.url().trim_end_matches('/'));
    let held = client::register(
        home.path(),
        &request("s-1", &[], Duration::hours(1)),
        "create-1",
        Utc::now(),
    )
    .unwrap()
    .retained
    .unwrap();
    store_cleartext_hub(home.path(), &hub, &held.instance);
    let before = double.lock().unwrap().requests;

    let error = client::register(
        home.path(),
        &request("s-2", &[], Duration::hours(1)),
        "create-2",
        Utc::now(),
    )
    .unwrap_err();
    assert!(
        error.contains("neither https nor http to a loopback"),
        "{error}"
    );
    assert!(
        !home
            .path()
            .join("instances/attempts/create-2.json")
            .exists(),
        "a refused registration kept an attempt"
    );
    let error = client::renew(
        home.path(),
        &held.instance,
        Utc::now() + Duration::hours(2),
        "renew-1",
        Utc::now(),
    )
    .unwrap_err();
    assert!(
        error.contains("neither https nor http to a loopback"),
        "{error}"
    );
    assert!(!home.path().join("instances/attempts/renew-1.json").exists());
    let error = client::status(home.path(), &held.instance, Utc::now()).unwrap_err();
    assert!(
        error.contains("neither https nor http to a loopback"),
        "{error}"
    );

    // The check the binding requires before a crossing cannot be made, so
    // the crossing is unavailable, as for a hub that cannot be reached.
    let mut server = mediating_server(home.path(), "s-1");
    let (is_error, text) = fetch(&mut server, &url);
    assert!(is_error, "{text}");
    assert!(text.contains("instance_check_unavailable"), "{text}");
    assert!(
        text.contains("neither https nor http to a loopback"),
        "{text}"
    );
    assert_eq!(*hits.lock().unwrap(), 0, "nothing reached the page");
    assert_eq!(double.lock().unwrap().requests, before, "nothing was sent");
    hub.stop();
    origin.stop();
}

// Catches: a session that loaded its key before the revocation was learnt
// signing on after it (docs/contracts/bot-identity.md §Revocation).
#[test]
fn an_open_session_stops_signing_once_another_process_records_a_revocation() {
    let home = tempfile::tempdir().unwrap();
    let (_double, mut hub) = edge_and_double(home.path());
    let (hits, mut origin) = page();
    let url = format!("{}/doc", origin.url().trim_end_matches('/'));
    let mut server = mediating_server(home.path(), "open");
    let (is_error, text) = fetch(&mut server, &url);
    assert!(!is_error, "{text}");
    let signed = *hits.lock().unwrap();
    assert!(signed > 0);

    // What a relay run or a session start in another process records on a
    // 401 for the enrolled ingest key.
    let mut record = EnrolmentRecord::load(home.path()).unwrap().unwrap();
    record
        .record_refusal(home.path(), "the member was removed")
        .unwrap();
    let (is_error, text) = fetch(&mut server, &url);
    assert!(is_error, "{text}");
    assert!(
        text.contains("could not be signed")
            && text.contains(
                "revoked by the hub: the member was removed after this session loaded it"
            ),
        "{text}"
    );
    assert_eq!(
        *hits.lock().unwrap(),
        signed,
        "the request was not sent unsigned"
    );
    hub.stop();
    origin.stop();
}
