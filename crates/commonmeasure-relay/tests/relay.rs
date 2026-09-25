//! The relay boundary and durability proofs, offline.
//!
//! The counterparty for the offline tests is a real HTTP server on loopback
//! (`commonmeasure_http::Server`), receiving the real bytes the relay posts; the proof
//! against a conforming receiver is the ignored live test in
//! `crates/commonmeasure-cli/tests/relay_e2e.rs`.

use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

fn crossing_line(event: &str, session: &str, url: &str, grounded: bool) -> String {
    json!({
        "seq": 0,
        "timestamp": "2026-08-02T10:00:00.000Z",
        "event": event,
        "payload": {
            "session_id": session,
            "timestamp": "2026-08-02T10:00:00.000Z",
            "mode": match event {
                "crossing_reconstructed" => "reconstructed",
                "crossing_mediated" | "crossing_refused" => "mediated",
                _ => "observed",
            },
            "host": "claude-code",
            "cwd": "/work/personal",
            "url": url,
            "host_name": "host.example",
            "content_hash": grounded.then_some(
                "sha256:aa044138653177b57cacad53fa4ea2be3b2770e7177198820949acad706450de"
            ),
            "grounded": grounded,
            "licence": {"state": "unknown"},
            "refusal": (event == "crossing_refused").then_some("host is on the denied list"),
            "derived_from": (event == "crossing_reconstructed")
                .then_some("/transcripts/old-session.jsonl"),
        },
    })
    .to_string()
}

fn write_session(home: &Path, session: &str, lines: &[String]) {
    if !home.join("policy.json").exists() {
        std::fs::write(
            home.join("policy.json"),
            r#"{"scopes":[{"match":"/work/personal","engagement":"personal","allow_telemetry_egress":true}]}"#,
        )
        .unwrap();
    }
    let dir = home.join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{session}.ndjson")),
        lines.join("\n") + "\n",
    )
    .unwrap();
}

fn crossing_in_cwd(event: &str, session: &str, url: &str, grounded: bool, cwd: &str) -> String {
    let mut record: Value =
        serde_json::from_str(&crossing_line(event, session, url, grounded)).unwrap();
    record["payload"]["cwd"] = json!(cwd);
    record.to_string()
}

/// A real receiver on loopback that accepts every batch and keeps the bodies.
fn accepting_receiver(bodies: Arc<Mutex<Vec<Value>>>) -> commonmeasure_http::ServerHandle {
    let server = commonmeasure_http::Server::bind("127.0.0.1:0").unwrap();
    server
        .spawn(move |request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let events = body["events"].as_array().map(Vec::len).unwrap_or(0);
            bodies.lock().unwrap().push(body);
            commonmeasure_http::Response::json(
                201,
                &json!({"status": "ok", "events_created": events}).to_string(),
            )
        })
        .unwrap()
}

/// Everything on disk that would make the batch undeliverable a second time:
/// the ids the relay considers burned, and what is left waiting in the spool.
fn burned_ids(home: &Path) -> Vec<String> {
    std::fs::read_to_string(home.join("relay/delivered.idx"))
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect()
}

/// A receiver that answers every batch with the same status and body.
fn fixed_answer_receiver(
    status: u16,
    content_type: &'static str,
    body: &'static str,
) -> commonmeasure_http::ServerHandle {
    let server = commonmeasure_http::Server::bind("127.0.0.1:0").unwrap();
    server
        .spawn(move |_| {
            let mut response = commonmeasure_http::Response::new(status, body.as_bytes().to_vec());
            response.headers.set("Content-Type", content_type);
            response
        })
        .unwrap()
}

#[test]
fn no_configured_receiver_means_no_egress_and_no_relay_state() {
    let home = tempfile::tempdir().unwrap();
    write_session(
        home.path(),
        "s-egress",
        &[crossing_line(
            "crossing_observed",
            "s-egress",
            "https://a.example/1",
            true,
        )],
    );

    let error =
        commonmeasure_relay::relay(home.path(), &commonmeasure_relay::RelayOptions::default())
            .expect_err("an unconfigured relay must refuse");
    let message = format!("{error:#}");
    assert!(
        message.contains("no telemetry receiver is configured"),
        "the refusal must name the missing configuration, got: {message}"
    );
    assert!(
        message.contains("nothing was projected and nothing was sent"),
        "the refusal must state that nothing left, got: {message}"
    );
    assert!(
        !home.path().join("relay").exists(),
        "refusing must leave no relay state behind"
    );

    let egress = commonmeasure_relay::egress_report(home.path());
    assert_eq!(egress["receiver"], Value::Null);
    assert_eq!(egress["delivered"], 0);
    assert_eq!(egress["pending"], 0);
}

#[test]
fn a_malformed_relay_config_is_an_error_rather_than_a_silent_absence() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("relay.json"), b"{not json").unwrap();

    let error =
        commonmeasure_relay::relay(home.path(), &commonmeasure_relay::RelayOptions::default())
            .expect_err("a malformed config must be an error");
    assert!(format!("{error:#}").contains("not a valid relay config"));

    let egress = commonmeasure_relay::egress_report(home.path());
    assert_eq!(egress["receiver"], Value::Null);
    assert!(
        egress["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("not a valid relay config"),
        "the console must report the broken config, not imply an unconfigured one"
    );
}

#[test]
fn client_engagements_default_to_no_egress_and_create_no_relay_state() {
    let home = tempfile::tempdir().unwrap();
    write_session(
        home.path(),
        "s-client",
        &[crossing_in_cwd(
            "crossing_observed",
            "s-client",
            "https://client-source.example/page",
            true,
            "/work/client-a",
        )],
    );
    std::fs::write(
        home.path().join("policy.json"),
        r#"{"scopes":[{"match":"/work/client-a","engagement":"client-a"}]}"#,
    )
    .unwrap();

    let report = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some("not-a-valid-receiver".to_owned()),
            ..Default::default()
        },
    )
    .expect("nothing cleared means the receiver is never contacted");
    assert_eq!(report.sessions_projected, 0);
    assert_eq!(report.events_enqueued, 0);
    assert_eq!(report.events_delivered, 0);
    assert!(burned_ids(home.path()).is_empty());
    assert!(
        !home.path().join("relay/spool/outbound.ndjson").exists(),
        "a denied crossing must never enter the spool"
    );
}

#[test]
fn one_session_filters_each_crossing_by_its_engagement() {
    let home = tempfile::tempdir().unwrap();
    write_session(
        home.path(),
        "s-worktrees",
        &[
            crossing_in_cwd(
                "crossing_observed",
                "s-worktrees",
                "https://personal.example/page",
                true,
                "/work/personal",
            ),
            crossing_in_cwd(
                "crossing_observed",
                "s-worktrees",
                "https://client.example/page",
                true,
                "/work/client-a",
            ),
        ],
    );
    std::fs::write(
        home.path().join("policy.json"),
        r#"{"scopes":[
            {"match":"/work/personal","engagement":"personal","allow_telemetry_egress":true},
            {"match":"/work/client-a","engagement":"client-a"}
        ]}"#,
    )
    .unwrap();

    let error = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some("not-a-valid-receiver".to_owned()),
            ..Default::default()
        },
    )
    .expect_err("the cleared crossing is spooled before delivery fails");
    assert!(format!("{error:#}").contains("remain spooled"));
    let spool = std::fs::read_to_string(home.path().join("relay/spool/outbound.ndjson")).unwrap();
    assert!(spool.contains("personal.example"));
    assert!(!spool.contains("client.example"));
}

#[test]
fn malformed_engagement_policy_refuses_before_projection_or_spooling() {
    let home = tempfile::tempdir().unwrap();
    write_session(
        home.path(),
        "s-broken-policy",
        &[crossing_line(
            "crossing_observed",
            "s-broken-policy",
            "https://source.example/page",
            true,
        )],
    );
    std::fs::write(home.path().join("policy.json"), "{not json").unwrap();

    let error = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some("not-a-valid-receiver".to_owned()),
            ..Default::default()
        },
    )
    .expect_err("broken policy cannot weaken to unfiltered egress");
    let message = format!("{error:#}");
    assert!(message.contains("is not a valid policy"), "{message}");
    assert!(
        message.contains("nothing was projected and nothing was sent"),
        "{message}"
    );
    assert!(!home.path().join("relay").exists());
}

/// The default that carries the whole filter: no declaration at all means
/// nothing leaves, and it must be the policy that says so rather than some
/// other absence. A receiver is configured here, so the only thing standing
/// between these crossings and the wire is the missing declaration.
#[test]
fn an_absent_policy_clears_nothing_and_creates_no_relay_state() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("s-undeclared.ndjson"),
        format!(
            "{}\n",
            crossing_line(
                "crossing_observed",
                "s-undeclared",
                "https://source.example/page",
                true,
            )
        ),
    )
    .unwrap();
    assert!(!home.path().join("policy.json").exists());

    let report = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some("not-a-valid-receiver".to_owned()),
            ..Default::default()
        },
    )
    .expect("nothing cleared means the receiver is never contacted");
    assert_eq!(report.sessions_projected, 0);
    assert_eq!(report.events_enqueued, 0);
    assert!(
        !home.path().join("relay/spool/outbound.ndjson").exists(),
        "an undeclared crossing must never enter the spool"
    );
}

/// A crossing's identity must not depend on what the policy says about the
/// crossings around it. If the filter handed projection a compacted list, an
/// event id would move whenever anything before it was excluded.
#[test]
fn a_crossing_keeps_its_event_ids_when_another_engagement_is_cleared() {
    let lines = [
        crossing_in_cwd(
            "crossing_observed",
            "s-identity",
            "https://client.example/page",
            true,
            "/work/client-a",
        ),
        crossing_in_cwd(
            "crossing_observed",
            "s-identity",
            "https://personal.example/page",
            true,
            "/work/personal",
        ),
    ];
    let personal_only = r#"{"scopes":[
        {"match":"/work/client-a","engagement":"client-a"},
        {"match":"/work/personal","engagement":"personal","allow_telemetry_egress":true}
    ]}"#;
    let both = r#"{"scopes":[
        {"match":"/work/client-a","engagement":"client-a","allow_telemetry_egress":true},
        {"match":"/work/personal","engagement":"personal","allow_telemetry_egress":true}
    ]}"#;

    let ids_for = |policy: &str| -> Vec<Value> {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("policy.json"), policy).unwrap();
        write_session(home.path(), "s-identity", &lines);
        commonmeasure_relay::relay(
            home.path(),
            &commonmeasure_relay::RelayOptions {
                receiver: Some("not-a-valid-receiver".to_owned()),
                ..Default::default()
            },
        )
        .expect_err("the cleared crossings spool before delivery fails");
        let spool =
            std::fs::read_to_string(home.path().join("relay/spool/outbound.ndjson")).unwrap();
        spool
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .flat_map(|entry| entry["document"]["events"].as_array().unwrap().clone())
            .filter(|event| event["content_url"] == json!("https://personal.example/page"))
            .map(|event| event["id"].clone())
            .collect()
    };

    let alone = ids_for(personal_only);
    assert_eq!(alone.len(), 2, "retrieved and grounded");
    assert_eq!(
        alone,
        ids_for(both),
        "the personal crossing changed identity because a different engagement was cleared: \
         an already-delivered event would redeliver under a new id, and the newly cleared \
         crossing would inherit the burned one and never be sent"
    );
}

/// Every delivered event is accounted for under exactly one clearance, so the
/// breakdown can never be read as a subset of what left.
fn clearance_total(report: &commonmeasure_relay::RelayReport) -> u64 {
    report
        .delivered_by_clearance
        .iter()
        .map(|delivered| delivered.events)
        .sum()
}

/// A published run is outside the session-engagement filter because the run
/// contract carries no engagement. Its events must therefore be reported as
/// that absence — not folded into the engagement that cleared the session
/// beside them, and not left out of an account that would then sum to less
/// than what was delivered.
#[test]
#[cfg_attr(
    not(evidence_output),
    ignore = "needs the committed runs: output/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn a_published_run_is_delivered_under_no_engagement_and_says_so() {
    let home = tempfile::tempdir().unwrap();
    write_session(
        home.path(),
        "s-beside-a-run",
        &[crossing_line(
            "crossing_observed",
            "s-beside-a-run",
            "https://a.example/1",
            true,
        )],
    );
    let run = Path::new(env!("COMMONMEASURE_EVIDENCE_DIR")).join("output/latest");

    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut receiver = accepting_receiver(bodies.clone());
    let report = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(receiver.url()),
            runs: vec![run],
            ..Default::default()
        },
    )
    .expect("relay");
    receiver.stop();

    assert_eq!(report.runs_projected, 1);
    let clearances: Vec<&commonmeasure_relay::Clearance> = report
        .delivered_by_clearance
        .iter()
        .map(|delivered| &delivered.clearance)
        .collect();
    assert_eq!(
        clearances,
        vec![
            &commonmeasure_relay::Clearance::GoverningEngagement("personal".to_owned()),
            &commonmeasure_relay::Clearance::RunWithoutEngagement,
        ],
        "the session's clearance and the run's absence of one are separate rows"
    );
    let under_run = report
        .delivered_by_clearance
        .iter()
        .find(|delivered| {
            delivered.clearance == commonmeasure_relay::Clearance::RunWithoutEngagement
        })
        .expect("the run's events are reported");
    assert!(
        under_run.events > 0,
        "the published run's admitted sources reached the receiver"
    );
    assert_eq!(clearance_total(&report), report.events_delivered);
}

/// The spool is the record of an egress decision already taken. A retry after
/// a failed delivery sends what was cleared then — correctly, because the
/// clearance happened at projection — but this run resolved nothing about it,
/// and it must not borrow a name from the policy as it now stands. Here the
/// operator withdrew the clearance in between: the report says one session was
/// withheld and that the events which still went out left under a clearance it
/// could not resolve.
#[test]
fn a_retry_after_a_withdrawn_clearance_names_no_engagement_it_did_not_resolve() {
    let home = tempfile::tempdir().unwrap();
    write_session(
        home.path(),
        "s-withdrawn",
        &[crossing_line(
            "crossing_observed",
            "s-withdrawn",
            "https://a.example/1",
            true,
        )],
    );

    // A receiver that answers 503, so the batch spools and delivery fails.
    let mut refusing = fixed_answer_receiver(503, "text/html", "<html>unavailable</html>");
    let error = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(refusing.url()),
            ..Default::default()
        },
    )
    .expect_err("a 503 is not an acceptance");
    refusing.stop();
    assert!(format!("{error:#}").contains("delivery to"), "{error:#}");

    // The operator withdraws the clearance before the retry.
    std::fs::write(
        home.path().join("policy.json"),
        r#"{"scopes":[{"match":"/work/personal","engagement":"personal"}]}"#,
    )
    .unwrap();

    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut receiver = accepting_receiver(bodies.clone());
    let report = relay_after_backoff(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(receiver.url()),
            ..Default::default()
        },
    )
    .expect("the spooled batch is delivered");
    receiver.stop();

    assert_eq!(report.events_enqueued, 0, "nothing new was cleared");
    assert_eq!(report.sessions_withheld, 1, "the session stayed home");
    assert_eq!(report.sessions_projected, 0);
    assert_eq!(
        report
            .delivered_by_clearance
            .iter()
            .map(|delivered| delivered.clearance.clone())
            .collect::<Vec<_>>(),
        vec![commonmeasure_relay::Clearance::SpooledBeforeThisRun],
        "an unresolved clearance is stated as one, never as the engagement the \
         policy happens to name now and never as none"
    );
    assert_eq!(clearance_total(&report), report.events_delivered);
    assert!(report.events_delivered > 0);
}

#[test]
fn a_reconstructed_crossing_is_never_projected_as_witnessed() {
    let home = tempfile::tempdir().unwrap();
    write_session(
        home.path(),
        "s-mixed",
        &[
            crossing_line(
                "crossing_observed",
                "s-mixed",
                "https://witnessed.example/page",
                true,
            ),
            crossing_line(
                "crossing_reconstructed",
                "s-mixed",
                "https://transcript-claim.example/page",
                true,
            ),
            crossing_line(
                "crossing_refused",
                "s-mixed",
                "https://refused.example/page",
                false,
            ),
        ],
    );
    let records =
        commonmeasure_harness::SessionLog::read(&home.path().join("sessions/s-mixed.ndjson"))
            .unwrap();
    let batches =
        commonmeasure_relay::project::project_session(None, "s-mixed", &records, &[], &|_| true)
            .batches;

    let urls: Vec<&str> = batches
        .iter()
        .flat_map(|batch| batch.events.iter())
        .map(|event| event.content_url.as_str())
        .collect();
    assert!(
        urls.iter()
            .all(|url| url.starts_with("https://witnessed.example/")),
        "only the witnessed crossing may project, got {urls:?}"
    );
    assert_eq!(
        urls.len(),
        2,
        "the witnessed crossing projects retrieval and grounding"
    );

    // A session known only from transcripts projects nothing at all.
    let reconstructed_only = [serde_json::from_str::<Value>(&crossing_line(
        "crossing_reconstructed",
        "s-transcripts",
        "https://transcript-claim.example/page",
        true,
    ))
    .unwrap()];
    assert!(
        commonmeasure_relay::project::project_session(
            None,
            "s-transcripts",
            &reconstructed_only,
            &[],
            &|_| true
        )
        .batches
        .is_empty(),
        "a transcript claim must not arrive at a publisher looking witnessed"
    );
}

#[test]
fn two_owners_projections_are_isolated_with_no_url_cross_contamination() {
    let owner_a = [
        crossing_line(
            "crossing_observed",
            "s-owner-a",
            "https://www.theguardian.com/a1",
            true,
        ),
        crossing_line(
            "crossing_observed",
            "s-owner-a",
            "https://www.theguardian.com/a2",
            false,
        ),
    ];
    let owner_b = [crossing_line(
        "crossing_mediated",
        "s-owner-b",
        "https://www.telegraph.co.uk/b1",
        true,
    )];
    let parse = |lines: &[String]| -> Vec<Value> {
        lines
            .iter()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    };
    let batches_a = commonmeasure_relay::project::project_session(
        None,
        "s-owner-a",
        &parse(&owner_a),
        &[],
        &|_| true,
    )
    .batches;
    let batches_b = commonmeasure_relay::project::project_session(
        None,
        "s-owner-b",
        &parse(&owner_b),
        &[],
        &|_| true,
    )
    .batches;

    let urls = |batches: &[commonmeasure_relay::wire::WireBatch]| -> Vec<String> {
        batches
            .iter()
            .flat_map(|batch| batch.events.iter())
            .map(|event| event.content_url.clone())
            .collect()
    };
    assert!(
        urls(&batches_a)
            .iter()
            .all(|url| url.contains("theguardian.com"))
    );
    assert!(
        urls(&batches_b)
            .iter()
            .all(|url| url.contains("telegraph.co.uk"))
    );

    // Different sessions, different envelopes, disjoint event identities:
    // nothing ties one owner's projection to the other's.
    assert_ne!(batches_a[0].session_id, batches_b[0].session_id);
    let ids_a: Vec<_> = batches_a
        .iter()
        .flat_map(|b| b.events.iter().map(|e| e.id))
        .collect();
    let ids_b: Vec<_> = batches_b
        .iter()
        .flat_map(|b| b.events.iter().map(|e| e.id))
        .collect();
    assert!(ids_a.iter().all(|id| !ids_b.contains(id)));
}

/// A crossing recorded under a named public-domain
/// internal prefix stays home after the operator removes the prefix. The
/// record is produced by the real capture path, so the classification under
/// test is the one capture actually writes; the relay-time home has no
/// policy.json at all, which is the prefix list at its emptiest.
#[test]
fn a_capture_time_internal_crossing_stays_home_after_the_prefix_is_removed() {
    let home = tempfile::tempdir().unwrap();
    let crossings = commonmeasure_harness::capture(
        &commonmeasure_harness::HookInput {
            session_id: Some("s-prefix-removed".into()),
            hook_event_name: Some("PostToolUse".into()),
            tool_name: Some("WebFetch".into()),
            tool_input: json!({"url": "https://intranet.example.com/private/handbook"}),
            tool_response: json!({"result": "internal handbook text"}),
            ..Default::default()
        },
        commonmeasure_harness::HostSurface::ClaudeCode,
        &["https://intranet.example.com/private/".to_owned()],
    );
    assert_eq!(
        crossings.len(),
        1,
        "the named prefix admits the crossing to the operator record"
    );
    let lines: Vec<String> = crossings
        .iter()
        .map(|crossing| {
            json!({
                "seq": 0,
                "timestamp": "2026-08-02T10:00:00.000Z",
                "event": "crossing_observed",
                "payload": crossing.to_record(),
            })
            .to_string()
        })
        .collect();
    write_session(home.path(), "s-prefix-removed", &lines);

    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut receiver = accepting_receiver(bodies.clone());
    let report = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(receiver.url()),
            ..Default::default()
        },
    )
    .expect("relay");
    assert_eq!(report.events_enqueued, 0, "nothing may be spooled");
    assert_eq!(report.events_delivered, 0, "nothing may cross the wire");
    assert!(
        bodies.lock().unwrap().is_empty(),
        "the receiver must see no trace of the internal crossing"
    );
    receiver.stop();
}

#[test]
fn a_relay_killed_before_acknowledging_redelivers_the_same_event_ids() {
    let home = tempfile::tempdir().unwrap();
    write_session(
        home.path(),
        "s-crash",
        &[
            crossing_line("crossing_observed", "s-crash", "https://a.example/1", true),
            crossing_line("crossing_observed", "s-crash", "https://a.example/2", false),
        ],
    );
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut receiver = accepting_receiver(bodies.clone());
    let options = commonmeasure_relay::RelayOptions {
        receiver: Some(receiver.url()),
        ..Default::default()
    };

    let first = commonmeasure_relay::relay(home.path(), &options).expect("first delivery");
    assert_eq!(first.events_delivered, 3);
    assert_eq!(first.batches_delivered, 1);

    // A second run of the unchanged ledger spools and sends nothing new.
    let second = commonmeasure_relay::relay(home.path(), &options).expect("second run");
    assert_eq!(second.events_enqueued, 0);
    assert_eq!(second.batches_delivered, 0);

    // The crash window: delivered, but killed before the acknowledgement was
    // recorded. Removing acceptance metadata injects that crash window.
    std::fs::remove_file(home.path().join("relay/spool/outbound.delivery.json")).unwrap();
    let redelivery = commonmeasure_relay::relay(home.path(), &options).expect("redelivery");
    assert_eq!(redelivery.events_delivered, 3, "the whole batch goes again");

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2, "one delivery, one redelivery");
    assert_eq!(
        bodies[0], bodies[1],
        "redelivery must carry byte-identical identity so the receiver counts once"
    );

    // The durable delivered count absorbed the duplicate.
    let egress = commonmeasure_relay::egress_report(home.path());
    assert_eq!(egress["delivered"], 3);
    assert_eq!(egress["pending"], 0);
    receiver.stop();
}

/// Relay one fresh two-event session to a receiver that answers every batch
/// with `status` and `body`, and return the result with the home it used.
fn relay_to_answer(
    status: u16,
    content_type: &'static str,
    body: &'static str,
) -> (
    tempfile::TempDir,
    anyhow::Result<commonmeasure_relay::RelayReport>,
) {
    let home = tempfile::tempdir().unwrap();
    write_session(
        home.path(),
        "s-answer",
        &[crossing_line(
            "crossing_observed",
            "s-answer",
            "https://a.example/1",
            true,
        )],
    );
    let mut receiver = fixed_answer_receiver(status, content_type, body);
    let result = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(receiver.url()),
            ..Default::default()
        },
    );
    receiver.stop();
    (home, result)
}

/// The batch left and its ids are recorded delivered, whatever the body said.
fn assert_accepted(home: &Path, report: &commonmeasure_relay::RelayReport) {
    assert_eq!(report.events_delivered, 2);
    assert_eq!(burned_ids(home).len(), 2);
    let egress = commonmeasure_relay::egress_report(home);
    assert_eq!(egress["delivered"], 2);
    assert_eq!(egress["pending"], 0);
}

/// Any 2xx is acceptance; a JSON body stating `events_created` gives the
/// count of newly recorded events.
#[test]
fn a_two_hundred_with_a_count_is_accepted_and_the_count_recorded() {
    let (home, result) = relay_to_answer(
        200,
        "application/json",
        "{\"status\":\"ok\",\"events_created\":2}",
    );
    let report = result.expect("200 is an acceptance");
    assert_accepted(home.path(), &report);
    assert_eq!(report.events_new_at_receiver, Some(2));
}

/// A receiver that queues the batch may answer 202 with no body. The batch is
/// delivered and the count of new events is unknown, never zero.
#[test]
fn a_two_hundred_and_two_without_a_body_is_accepted_with_the_count_unknown() {
    let (home, result) = relay_to_answer(202, "application/json", "");
    let report = result.expect("202 is an acceptance");
    assert_accepted(home.path(), &report);
    assert_eq!(report.events_new_at_receiver, None);
}

#[test]
fn a_two_hundred_and_four_is_accepted_with_the_count_unknown() {
    let (home, result) = relay_to_answer(204, "application/json", "");
    let report = result.expect("204 is an acceptance");
    assert_accepted(home.path(), &report);
    assert_eq!(report.events_new_at_receiver, None);
}

/// A 2xx whose body is not JSON, or is JSON without an unsigned
/// `events_created`, is still an acceptance; only the count is unknown.
#[test]
fn a_two_hundred_with_a_malformed_body_is_accepted_with_the_count_unknown() {
    for (content_type, body) in [
        (
            "text/html; charset=utf-8",
            "<html><body>stored</body></html>",
        ),
        ("application/json", "{\"status\":\"ok\",\"events_created\":"),
        ("application/json", "{\"status\":\"ok\"}"),
        ("application/json", "{\"events_created\":-1}"),
    ] {
        let (home, result) = relay_to_answer(201, content_type, body);
        let report = result.unwrap_or_else(|error| panic!("{body}: {error:#}"));
        assert_accepted(home.path(), &report);
        assert_eq!(report.events_new_at_receiver, None, "{body}");
    }
}

/// A non-2xx fails the delivery: no id is burned, the batch stays spooled and
/// the failure names the receiver's detail.
#[test]
fn a_non_two_hundred_is_a_delivery_failure_and_keeps_the_batch() {
    let (home, result) =
        relay_to_answer(400, "application/json", "{\"detail\":\"batch rejected\"}");
    let error = result.expect_err("400 is no acceptance");
    let message = format!("{error:#}");
    assert!(
        message.contains("receiver answered 400: batch rejected"),
        "{message}"
    );
    assert!(message.contains("remain spooled"), "{message}");
    assert!(burned_ids(home.path()).is_empty());
    let egress = commonmeasure_relay::egress_report(home.path());
    assert_eq!(egress["delivered"], 0);
    assert_eq!(egress["pending"], 2);
}

/// The run's total of new events is unknown when any batch was accepted
/// without a count; a partial sum would read as the whole.
#[test]
fn one_batch_without_a_count_makes_the_run_total_unknown() {
    let home = tempfile::tempdir().unwrap();
    for session in ["s-counted", "s-uncounted"] {
        write_session(
            home.path(),
            session,
            &[crossing_line(
                "crossing_observed",
                session,
                "https://a.example/1",
                true,
            )],
        );
    }
    let answered = Arc::new(Mutex::new(0usize));
    let server = commonmeasure_http::Server::bind("127.0.0.1:0").unwrap();
    let mut receiver = server
        .spawn(move |_| {
            let mut answered = answered.lock().unwrap();
            *answered += 1;
            if *answered == 1 {
                commonmeasure_http::Response::json(200, r#"{"events_created":2}"#)
            } else {
                commonmeasure_http::Response::new(204, Vec::new())
            }
        })
        .unwrap();
    let report = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(receiver.url()),
            ..Default::default()
        },
    )
    .expect("both batches are accepted");
    receiver.stop();

    assert_eq!(report.batches_delivered, 2);
    assert_eq!(report.events_delivered, 4);
    assert_eq!(report.events_new_at_receiver, None);
}

/// A credential the operator embedded in a fetched URL is authentication, not
/// content identity: it reaches neither the receiver nor the on-disk spool,
/// while the rest of the URL crosses as witnessed.
#[test]
fn an_embedded_credential_reaches_neither_the_wire_nor_the_spool() {
    let home = tempfile::tempdir().unwrap();
    write_session(
        home.path(),
        "s-credential",
        &[crossing_line(
            "crossing_observed",
            "s-credential",
            "https://user:hunter2@api.example.com/v1/data?api_key=sk-live-SECRET123",
            true,
        )],
    );
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut receiver = accepting_receiver(bodies.clone());
    commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(receiver.url()),
            ..Default::default()
        },
    )
    .expect("relay");
    receiver.stop();

    let bodies = bodies.lock().unwrap();
    let urls: Vec<&str> = bodies[0]["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["content_url"].as_str().unwrap())
        .collect();
    assert!(
        urls.iter()
            .all(|url| *url == "https://api.example.com/v1/data?api_key=sk-live-SECRET123"),
        "the userinfo must not cross the boundary, got {urls:?}"
    );

    let spooled = std::fs::read_to_string(home.path().join("relay/spool/outbound.ndjson")).unwrap();
    assert!(
        !spooled.contains("hunter2"),
        "the credential must not be persisted in the spool"
    );
}

#[test]
fn a_failed_delivery_leaves_durable_inspectable_spool_state() {
    let home = tempfile::tempdir().unwrap();
    write_session(
        home.path(),
        "s-fail",
        &[crossing_line(
            "crossing_observed",
            "s-fail",
            "https://a.example/1",
            true,
        )],
    );
    // A real socket that answers 503 to everything.
    let server = commonmeasure_http::Server::bind("127.0.0.1:0").unwrap();
    let mut handle = server
        .spawn(|_| {
            commonmeasure_http::Response::json(503, &json!({"detail": "unavailable"}).to_string())
        })
        .unwrap();
    let options = commonmeasure_relay::RelayOptions {
        receiver: Some(handle.url()),
        ..Default::default()
    };

    let error = commonmeasure_relay::relay(home.path(), &options).expect_err("delivery must fail");
    assert!(format!("{error:#}").contains("remain spooled"));

    // The spool holds the batch, the account holds the failure, and a later
    // relay against a working receiver delivers exactly what was spooled.
    let egress = commonmeasure_relay::egress_report(home.path());
    assert_eq!(egress["delivered"], 0);
    assert_eq!(egress["pending"], 2);
    assert!(
        egress["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("Last attempt failed"),
        "the console must report the failure, got {}",
        egress["detail"]
    );
    handle.stop();

    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut working = accepting_receiver(bodies.clone());
    let recovered = relay_after_backoff(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(working.url()),
            ..Default::default()
        },
    )
    .expect("recovery");
    assert_eq!(recovered.events_delivered, 2);
    assert_eq!(
        recovered.events_enqueued, 0,
        "recovery drains the spool, not a reprojection"
    );
    working.stop();
}

/// The remedy `docs/contracts/telemetry-projection.md` §Delivery state gives for a
/// damaged line of an undelivered batch, after a clean close (the journal
/// holds its header alone): the line is emptied with its newline kept and its
/// snapshot entry removed, and the next run projects the session's events
/// again under the same ids.
#[test]
fn emptying_a_damaged_undelivered_line_and_removing_its_state_projects_the_events_again() {
    let home = tempfile::tempdir().unwrap();
    write_session(
        home.path(),
        "s-damaged",
        &[crossing_line(
            "crossing_observed",
            "s-damaged",
            "https://a.example/1",
            true,
        )],
    );
    let server = commonmeasure_http::Server::bind("127.0.0.1:0").unwrap();
    let mut refusing = server
        .spawn(|_| {
            commonmeasure_http::Response::json(503, &json!({"detail": "unavailable"}).to_string())
        })
        .unwrap();
    commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(refusing.url()),
            ..Default::default()
        },
    )
    .expect_err("delivery must fail");
    refusing.stop();

    let spool = home.path().join("relay/spool");
    let queue = std::fs::read(spool.join("outbound.ndjson")).unwrap();
    let spooled_ids = |queue: &[u8]| -> Vec<String> {
        let entry: Value = serde_json::from_slice(queue).unwrap();
        entry["document"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["id"].as_str().unwrap().to_owned())
            .collect()
    };
    let first_ids = spooled_ids(&queue);
    std::fs::write(
        spool.join("outbound.ndjson"),
        [&queue[..queue.len() / 2], b"\n"].concat(),
    )
    .unwrap();

    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut working = accepting_receiver(bodies.clone());
    let options = commonmeasure_relay::RelayOptions {
        receiver: Some(working.url()),
        ..Default::default()
    };
    let error = relay_after_backoff(home.path(), &options).expect_err("the line is damaged");
    assert!(
        format!("{error:#}").contains("parse spool line 0; nothing is delivered"),
        "{error:#}"
    );
    assert!(bodies.lock().unwrap().is_empty());

    std::fs::write(spool.join("outbound.ndjson"), "\n").unwrap();
    let snapshot_path = spool.join("outbound.delivery.json");
    let mut snapshot: Value =
        serde_json::from_slice(&std::fs::read(&snapshot_path).unwrap()).unwrap();
    assert!(snapshot.as_object_mut().unwrap().remove("0").is_some());
    std::fs::write(&snapshot_path, snapshot.to_string()).unwrap();

    let recovered = relay_after_backoff(home.path(), &options).expect("recovery");
    working.stop();
    assert_eq!(
        recovered.events_enqueued, 2,
        "the events are projected again"
    );
    assert_eq!(recovered.events_delivered, 2);
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1);
    let mut sent: Vec<String> = bodies[0]["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["id"].as_str().unwrap().to_owned())
        .collect();
    let mut first_ids = first_ids;
    sent.sort();
    first_ids.sort();
    assert_eq!(sent, first_ids);
}

/// Both remedies of §Delivery state followed to the letter. Five batches:
/// delivered, queued after a refusal (damaged), delivered (damaged), queued
/// after a refusal, and enqueued but never claimed. After the repair every
/// other batch has the state it had, no delivered batch is sent again, and
/// each undelivered batch reaches the receiver once. The receiver is a
/// loopback double that refuses by host.
#[test]
fn the_damaged_line_remedies_leave_every_other_batch_in_place() {
    use commonmeasure_relay::spool::{DeliveryStatus, Spool};
    let home = tempfile::tempdir().unwrap();
    let sessions = ["p0", "p1", "p2", "p3", "p4"];
    for session in sessions {
        write_session(
            home.path(),
            session,
            &[crossing_line(
                "crossing_observed",
                session,
                &format!("https://{session}.example/page"),
                true,
            )],
        );
    }
    let refused: Arc<Mutex<Vec<&str>>> = Arc::new(Mutex::new(vec!["p1.", "p3.", "p4."]));
    let bodies = Arc::new(Mutex::new(Vec::<Value>::new()));
    let mut receiver = {
        let (refused, bodies) = (refused.clone(), bodies.clone());
        commonmeasure_http::Server::bind("127.0.0.1:0")
            .unwrap()
            .spawn(move |request| {
                let text = String::from_utf8_lossy(&request.body).into_owned();
                if refused
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|host| text.contains(host))
                {
                    return commonmeasure_http::Response::text(503, "refused");
                }
                bodies
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str(&text).unwrap());
                commonmeasure_http::Response::json(201, r#"{"status":"ok","events_created":2}"#)
            })
            .unwrap()
    };
    // Named, so that the batches are spooled in this order.
    let options = commonmeasure_relay::RelayOptions {
        receiver: Some(receiver.url()),
        sessions: sessions
            .iter()
            .map(|session| (*session).to_owned())
            .collect(),
        ..Default::default()
    };
    commonmeasure_relay::relay(home.path(), &options).expect_err("three batches are refused");

    // The last batch as a relay killed between the enqueue and the claim
    // left it: no metadata.
    let spool = home.path().join("relay/spool");
    let queue_path = spool.join("outbound.ndjson");
    let snapshot_path = spool.join("outbound.delivery.json");
    let written: Vec<String> = std::fs::read_to_string(&queue_path)
        .unwrap()
        .lines()
        .map(|line| {
            assert!(line.starts_with(r#"{"index":"#), "{line}");
            line.to_owned()
        })
        .collect();
    assert_eq!(written.len(), 5);
    let write_queue = |lines: &[String]| {
        std::fs::write(&queue_path, lines.join("\n") + "\n").unwrap();
    };
    let remove_state = |index: &str| {
        let mut snapshot: Value =
            serde_json::from_slice(&std::fs::read(&snapshot_path).unwrap()).unwrap();
        assert!(snapshot.as_object_mut().unwrap().remove(index).is_some());
        std::fs::write(&snapshot_path, snapshot.to_string()).unwrap();
    };
    remove_state("4");
    assert_eq!(
        std::fs::read_to_string(spool.join("outbound.delivery.journal"))
            .unwrap()
            .lines()
            .count(),
        1,
        "after a clean close the journal names no batch"
    );
    let other_states = |home: &Path| {
        let mut states = Spool::read_only(home).delivery_states().unwrap();
        states.remove(&1);
        states.retain(|index, _| *index < 5);
        serde_json::to_value(states).unwrap()
    };
    let before = other_states(home.path());
    assert_eq!(
        before
            .as_object()
            .unwrap()
            .iter()
            .map(|(index, state)| format!("{index} {}", state["status"].as_str().unwrap()))
            .collect::<Vec<_>>(),
        ["0 delivered", "2 delivered", "3 queued", "4 queued"]
    );
    assert_eq!(before["3"]["attempts"], 1);
    assert_eq!(before["4"]["attempts"], 0);

    let mut damaged = written.clone();
    for line in [1, 2] {
        damaged[line].truncate(written[line].len() / 2);
    }
    write_queue(&damaged);
    refused.lock().unwrap().clear();
    bodies.lock().unwrap().clear();
    let error = relay_after_backoff(home.path(), &options).expect_err("line 1 is damaged");
    assert!(
        format!("{error:#}").contains("parse spool line 1; nothing is delivered"),
        "{error:#}"
    );

    // The undelivered batch: the line emptied and its snapshot entry
    // removed.
    damaged[1].clear();
    write_queue(&damaged);
    remove_state("1");
    let error = relay_after_backoff(home.path(), &options).expect_err("line 2 is damaged");
    assert!(
        format!("{error:#}").contains("parse spool line 2; nothing is delivered"),
        "{error:#}"
    );
    assert!(bodies.lock().unwrap().is_empty());

    // The delivered batch: the contract's placeholder, which keeps the index.
    damaged[2] = r#"{"index":2,"origin":"damaged line replaced","directory_selection":false,"document":{"events":[]}}"#
        .to_owned();
    write_queue(&damaged);
    assert_eq!(other_states(home.path()), before);

    let recovered = relay_after_backoff(home.path(), &options).expect("recovery");
    receiver.stop();
    assert_eq!(
        recovered.events_enqueued, 2,
        "the emptied batch's events are projected again, and no others"
    );
    assert_eq!(recovered.batches_delivered, 3);
    let mut sent = delivered_urls(&bodies);
    sent.sort();
    sent.dedup();
    assert_eq!(
        sent,
        [
            "https://p1.example/page",
            "https://p3.example/page",
            "https://p4.example/page"
        ],
        "every undelivered batch leaves, and no delivered batch again"
    );
    assert_eq!(bodies.lock().unwrap().len(), 3, "each once");
    let states = Spool::read_only(home.path()).delivery_states().unwrap();
    assert_eq!(states.keys().copied().collect::<Vec<_>>(), [0, 2, 3, 4, 5]);
    assert!(
        states
            .values()
            .all(|state| state.status == DeliveryStatus::Delivered)
    );
    let after = other_states(home.path());
    for delivered in ["0", "2"] {
        assert_eq!(after[delivered], before[delivered]);
    }
}

/// Every delivery names the delivering software in its User-Agent: the
/// receiver records the product token per batch, which is what lets a fleet
/// view answer which edges run what.
#[test]
fn a_delivery_carries_the_relay_product_token() {
    let agents = Arc::new(Mutex::new(Vec::<String>::new()));
    let capture = agents.clone();
    let server = commonmeasure_http::Server::bind("127.0.0.1:0").unwrap();
    let mut handle = server
        .spawn(move |request| {
            capture.lock().unwrap().push(
                request
                    .headers
                    .get("User-Agent")
                    .unwrap_or_default()
                    .to_owned(),
            );
            commonmeasure_http::Response::json(
                201,
                &json!({"status": "ok", "events_created": 0}).to_string(),
            )
        })
        .unwrap();

    commonmeasure_relay::client::deliver(
        &handle.url(),
        Some("key"),
        &json!({"document_type": "event_batch"}),
    )
    .expect("delivery");
    handle.stop();

    assert_eq!(
        agents.lock().unwrap().as_slice(),
        [commonmeasure_relay::client::USER_AGENT],
        "the delivery must carry the relay's product token"
    );
    assert!(
        commonmeasure_relay::client::USER_AGENT.starts_with("commonmeasure-relay/"),
        "the token names the software before its version"
    );
}

/// A search result a supplier served names the supplier in the retrieval
/// event's namespaced field, and a fetch this edge made names none. The
/// receiver sees the field on the posted bytes; the operator's own corpus is
/// never named.
#[test]
fn a_supplied_result_names_its_supplier_at_the_receiver_and_a_fetch_does_not() {
    let home = tempfile::tempdir().unwrap();
    let mut supplied: Value = serde_json::from_str(&crossing_line(
        "crossing_mediated",
        "s1",
        "https://host.example/served",
        false,
    ))
    .unwrap();
    supplied["payload"]["supplier"] = json!("ozone");
    let mut internal: Value = serde_json::from_str(&crossing_line(
        "crossing_mediated",
        "s1",
        "https://host.example/own",
        false,
    ))
    .unwrap();
    internal["payload"]["supplier"] = json!("internal");
    let fetched = crossing_line(
        "crossing_mediated",
        "s1",
        "https://host.example/fetched",
        true,
    );
    write_session(
        home.path(),
        "s1",
        &[supplied.to_string(), internal.to_string(), fetched],
    );

    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut receiver = accepting_receiver(bodies.clone());
    commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(receiver.url()),
            api_key: Some("key".to_owned()),
            runs: Vec::new(),
            sessions: Vec::new(),
            ..Default::default()
        },
    )
    .expect("relay");
    receiver.stop();

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1);
    let field = commonmeasure_relay::project::SUPPLIER_FIELD;
    let by_url: Vec<(String, Option<String>)> = bodies[0]["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| {
            (
                event["content_url"].as_str().unwrap().to_owned(),
                event["data"][field].as_str().map(str::to_owned),
            )
        })
        .collect();
    assert_eq!(
        by_url,
        vec![
            (
                "https://host.example/served".to_owned(),
                Some("ozone".to_owned())
            ),
            ("https://host.example/own".to_owned(), None),
            ("https://host.example/fetched".to_owned(), None),
            ("https://host.example/fetched".to_owned(), None),
        ],
        "the supplier is named on the supplied result alone"
    );
}

fn registered_line(url: &str, grounded: bool, issuer: &str, revision: i64) -> String {
    let mut record: Value =
        serde_json::from_str(&crossing_line("crossing_mediated", "s", url, grounded)).unwrap();
    record["payload"]["instance"] = json!({
        "issuer": issuer,
        "id": "0b6f6c0e-5d0a-4a57-9f0e-3f1c2d4e5a6b",
        "revision": revision,
    });
    record.to_string()
}

fn instance_members(body: &Value) -> Vec<(String, String, Value)> {
    body["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| {
            (
                event["type"].as_str().unwrap().to_owned(),
                event["content_url"].as_str().unwrap_or("").to_owned(),
                event.get("instance").cloned().unwrap_or(Value::Null),
            )
        })
        .collect()
}

/// The instance reference reaches the receiver that issued it, on the content
/// events of registered records only, at each record's own revision; an
/// unregistered session's batch is unchanged; and the same log relayed to a
/// receiver that did not issue the instance carries no member. Both receivers
/// are loopback servers recording what the relay posts, not a hub: this
/// establishes what is sent, not that a hub records it.
#[test]
fn the_instance_reference_reaches_its_issuer_alone_at_each_records_revision() {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut hub = accepting_receiver(bodies.clone());
    // The receiver `relay.json` holds after enrolment is the hub's origin
    // and its telemetry path; the recorded issuer is the origin alone.
    let issuer = hub.url().trim_end_matches('/').to_owned();
    let lines = |issuer: &str| {
        vec![
            crossing_line(
                "crossing_mediated",
                "s",
                "https://host.example/before",
                true,
            ),
            // A boundary written under the registration still carries none.
            {
                let mut boundary: Value = serde_json::from_str(&turn_line(
                    "turn_started",
                    Some("t1"),
                    Some("/work/personal"),
                ))
                .unwrap();
                boundary["payload"]["instance"] = json!({"issuer": issuer, "id": "0b6f6c0e-5d0a-4a57-9f0e-3f1c2d4e5a6b", "revision": 1});
                boundary.to_string()
            },
            registered_line("https://host.example/first", true, issuer, 1),
            // Renewed between the two records.
            registered_line("https://host.example/renewed", false, issuer, 2),
        ]
    };
    let relay_to = |home: &Path, receiver: String| {
        commonmeasure_relay::relay(
            home,
            &commonmeasure_relay::RelayOptions {
                receiver: Some(receiver),
                api_key: Some("key".to_owned()),
                ..Default::default()
            },
        )
        .expect("relay")
    };

    let home = tempfile::tempdir().unwrap();
    write_session(home.path(), "registered", &lines(&issuer));
    write_session(
        home.path(),
        "unregistered",
        &[crossing_line(
            "crossing_mediated",
            "s",
            "https://host.example/plain",
            true,
        )],
    );
    relay_to(home.path(), format!("{issuer}/api/v1/telemetry"));
    hub.stop();

    let reference = |revision: i64| json!({"issuer": issuer, "id": "0b6f6c0e-5d0a-4a57-9f0e-3f1c2d4e5a6b", "revision": revision});
    let retrieved = "content_retrieved".to_owned();
    let grounded = "content_grounded".to_owned();
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    let registered = bodies
        .iter()
        .find(|body| body["events"].as_array().unwrap().len() > 2)
        .expect("the registered session's batch");
    let url = |path: &str| format!("https://host.example/{path}");
    assert_eq!(
        instance_members(registered),
        vec![
            (retrieved.clone(), url("before"), Value::Null),
            (grounded.clone(), url("before"), Value::Null),
            ("turn_started".to_owned(), String::new(), Value::Null),
            (retrieved.clone(), url("first"), reference(1)),
            (grounded.clone(), url("first"), reference(1)),
            (retrieved.clone(), url("renewed"), reference(2)),
        ]
    );
    let unregistered = bodies
        .iter()
        .find(|body| body["events"].as_array().unwrap().len() == 2)
        .expect("the unregistered session's batch");
    assert!(
        !unregistered.to_string().contains("instance"),
        "a session with no registration projects nothing new"
    );

    // The same log, relayed to a receiver the instance was not issued by.
    let other_bodies = Arc::new(Mutex::new(Vec::new()));
    let mut other = accepting_receiver(other_bodies.clone());
    let other_home = tempfile::tempdir().unwrap();
    write_session(other_home.path(), "registered", &lines(&issuer));
    relay_to(other_home.path(), other.url());
    other.stop();
    let other_bodies = other_bodies.lock().unwrap();
    assert_eq!(other_bodies.len(), 1);
    assert_eq!(other_bodies[0]["events"].as_array().unwrap().len(), 6);
    assert!(
        !other_bodies[0].to_string().contains("instance"),
        "the reference leaves only for its issuer"
    );
}

/// A batch queued for the hub that issued the instance, then delivered after
/// the receiver changed. Each home spools one batch against the issuer while
/// it answers 503, so the spooled document carries the member. Delivered to
/// another receiver by `--receiver`, and by the `relay.json` a second
/// enrolment writes, the posted events carry no member; delivered to the
/// issuer once it accepts, they carry it at each record's revision. The
/// servers are loopback receivers recording what the relay posts, not hubs.
#[test]
fn a_spooled_batch_delivered_to_another_receiver_loses_the_instance_reference() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let accepting = Arc::new(AtomicBool::new(false));
    let issuer_bodies = Arc::new(Mutex::new(Vec::new()));
    let mut hub = {
        let accepting = accepting.clone();
        let bodies = issuer_bodies.clone();
        commonmeasure_http::Server::bind("127.0.0.1:0")
            .unwrap()
            .spawn(move |request| {
                if !accepting.load(Ordering::SeqCst) {
                    return commonmeasure_http::Response::json(
                        503,
                        &json!({"detail": "unavailable"}).to_string(),
                    );
                }
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                let events = body["events"].as_array().map_or(0, Vec::len);
                bodies.lock().unwrap().push(body);
                commonmeasure_http::Response::json(
                    201,
                    &json!({"status": "ok", "events_created": events}).to_string(),
                )
            })
            .unwrap()
    };
    let issuer = hub.url().trim_end_matches('/').to_owned();
    let enrolled_with = |home: &Path, receiver: &str| {
        std::fs::write(
            home.join("relay.json"),
            json!({"receiver": receiver, "api_key": "key"}).to_string(),
        )
        .unwrap();
    };
    let spooled_for_the_issuer = || {
        let home = tempfile::tempdir().unwrap();
        write_session(
            home.path(),
            "registered",
            &[
                registered_line("https://host.example/first", true, &issuer, 1),
                registered_line("https://host.example/renewed", false, &issuer, 2),
            ],
        );
        enrolled_with(home.path(), &format!("{issuer}/api/v1/telemetry"));
        commonmeasure_relay::relay(home.path(), &Default::default())
            .expect_err("the issuer answers 503");
        let pending = commonmeasure_relay::spool::Spool::open(home.path())
            .unwrap()
            .pending()
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].1.document["events"][0]["instance"]["issuer"],
            json!(issuer),
            "the batch was queued for the issuer"
        );
        home
    };
    let members = |body: &Value| -> Vec<Value> {
        instance_members(body)
            .into_iter()
            .map(|(_, _, member)| member)
            .collect()
    };
    // The count the delivery loop recorded on the spool state. It is the
    // retention exemption's only input: a batch that records none is
    // released by the count rule, and its members with it.
    let recorded_withheld = |home: &Path| -> Vec<Option<u64>> {
        commonmeasure_relay::spool::Spool::read_only(home)
            .delivery_states()
            .unwrap()
            .values()
            .map(|state| {
                assert_eq!(
                    state.status,
                    commonmeasure_relay::spool::DeliveryStatus::Delivered
                );
                state.instance_references_withheld
            })
            .collect()
    };

    let other_bodies = Arc::new(Mutex::new(Vec::new()));
    let mut other = accepting_receiver(other_bodies.clone());

    let by_flag = spooled_for_the_issuer();
    let report = relay_after_backoff(
        by_flag.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(other.url()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(report.events_delivered, 3);
    assert_eq!(
        report.instance_references_withheld, 3,
        "the report counts the references a re-targeted delivery withheld"
    );
    assert_eq!(recorded_withheld(by_flag.path()), [Some(3)]);

    let by_enrolment = spooled_for_the_issuer();
    enrolled_with(
        by_enrolment.path(),
        &format!("{}/api/v1/telemetry", other.url().trim_end_matches('/')),
    );
    let report = relay_after_backoff(by_enrolment.path(), &Default::default()).unwrap();
    assert_eq!(report.events_delivered, 3);
    assert_eq!(
        report.instance_references_withheld, 3,
        "the report counts the references a re-targeted delivery withheld"
    );
    assert_eq!(recorded_withheld(by_enrolment.path()), [Some(3)]);
    other.stop();

    let other_bodies = other_bodies.lock().unwrap();
    assert_eq!(other_bodies.len(), 2);
    for body in other_bodies.iter() {
        assert_eq!(body["events"].as_array().unwrap().len(), 3);
        assert!(
            !body.to_string().contains("instance"),
            "a receiver that did not issue the instance is sent no reference: {body}"
        );
    }

    let kept = spooled_for_the_issuer();
    accepting.store(true, Ordering::SeqCst);
    let report = relay_after_backoff(kept.path(), &Default::default()).unwrap();
    assert_eq!(report.instance_references_withheld, 0);
    assert_eq!(recorded_withheld(kept.path()), [Some(0)]);
    hub.stop();
    let reference = |revision: i64| json!({"issuer": issuer, "id": "0b6f6c0e-5d0a-4a57-9f0e-3f1c2d4e5a6b", "revision": revision});
    let issuer_bodies = issuer_bodies.lock().unwrap();
    assert_eq!(issuer_bodies.len(), 1);
    assert_eq!(
        members(&issuer_bodies[0]),
        vec![reference(1), reference(1), reference(2)]
    );
}

/// Kill and recover, offline. A registered session's batch is claimed and
/// posted, the issuer accepts it, and the process stops before it records the
/// answer: the durable state a `SIGKILL` between the spool claim and the
/// receiver's answer leaves. That state is made here by claiming through the
/// spool and posting through the relay's client without recording an outcome.
/// A later run waits for the claim's persisted deadline, then sends the same
/// event ids with the same `instance` members, and records them delivered
/// once. The issuer is a loopback double that counts an event id as created
/// once, as the delivery contract asks of a receiver; it is not a hub, so
/// this establishes what the edge sends and records and nothing about a hub's
/// duty read.
#[test]
fn a_batch_accepted_before_the_relay_was_killed_is_sent_again_with_the_same_ids_and_reference() {
    use commonmeasure_relay::spool::{DeliveryStatus, Spool};
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicBool, Ordering};

    let accepting = Arc::new(AtomicBool::new(false));
    let bodies = Arc::new(Mutex::new(Vec::<Value>::new()));
    let known = Arc::new(Mutex::new(BTreeSet::<String>::new()));
    let mut issuer = {
        let (accepting, bodies, known) = (accepting.clone(), bodies.clone(), known.clone());
        commonmeasure_http::Server::bind("127.0.0.1:0")
            .unwrap()
            .spawn(move |request| {
                if !accepting.load(Ordering::SeqCst) {
                    return commonmeasure_http::Response::json(
                        503,
                        &json!({"detail": "unavailable"}).to_string(),
                    );
                }
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                let created = body["events"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|event| event["id"].as_str())
                    .filter(|id| known.lock().unwrap().insert((*id).to_owned()))
                    .count();
                bodies.lock().unwrap().push(body);
                commonmeasure_http::Response::json(
                    201,
                    &json!({"status": "ok", "events_created": created}).to_string(),
                )
            })
            .unwrap()
    };
    let origin = issuer.url().trim_end_matches('/').to_owned();
    let receiver = format!("{origin}/api/v1/telemetry");
    let home = tempfile::tempdir().unwrap();
    write_session(
        home.path(),
        "registered",
        &[
            registered_line("https://host.example/first", true, &origin, 1),
            registered_line("https://host.example/renewed", false, &origin, 2),
        ],
    );
    std::fs::write(
        home.path().join("relay.json"),
        json!({"receiver": receiver, "api_key": "key"}).to_string(),
    )
    .unwrap();

    // The session's process is gone and nothing was relayed: the record alone
    // holds the work. The first run spools it while the issuer is down.
    let start = chrono::Utc::now();
    commonmeasure_relay::relay_with_clock(home.path(), &Default::default(), &|| start)
        .expect_err("the issuer answers 503");
    let due = Spool::read_only(home.path()).delivery_states().unwrap()[&0]
        .next_attempt_at
        .expect("a persisted deadline");

    // The killed run: claim, post, accepted, and no outcome recorded.
    accepting.store(true, Ordering::SeqCst);
    {
        let spool = Spool::open(home.path()).unwrap();
        assert!(spool.claim(0, due).unwrap());
        let (_, entry) = spool.pending().unwrap().remove(0);
        let accepted =
            commonmeasure_relay::client::deliver(&receiver, Some("key"), &entry.document).unwrap();
        assert_eq!(accepted.events_created, Some(3));
    }
    let claimed = Spool::read_only(home.path()).delivery_states().unwrap()[&0].clone();
    assert_eq!(
        (claimed.attempts, claimed.status),
        (2, DeliveryStatus::Queued)
    );
    assert!(
        burned_ids(home.path()).is_empty(),
        "the edge never learnt of it"
    );
    let deadline = claimed.next_attempt_at.expect("the claim's deadline");

    let early = deadline - chrono::Duration::milliseconds(1);
    let report =
        commonmeasure_relay::relay_with_clock(home.path(), &Default::default(), &|| early).unwrap();
    assert_eq!(
        report.batches_delivered, 0,
        "not before the persisted deadline"
    );
    assert_eq!(bodies.lock().unwrap().len(), 1);

    let report =
        commonmeasure_relay::relay_with_clock(home.path(), &Default::default(), &|| deadline)
            .unwrap();
    assert_eq!((report.batches_delivered, report.events_delivered), (1, 3));
    assert_eq!(
        report.events_new_at_receiver,
        Some(0),
        "the issuer held every id already"
    );
    assert_eq!(report.instance_references_withheld, 0);
    let again =
        commonmeasure_relay::relay_with_clock(home.path(), &Default::default(), &|| deadline)
            .unwrap();
    assert_eq!((again.events_enqueued, again.batches_delivered), (0, 0));
    issuer.stop();

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2, "the killed post and one recovery post");
    assert_eq!(
        instance_members(&bodies[0]),
        instance_members(&bodies[1]),
        "the same event ids with the same instance members"
    );
    assert_eq!(instance_members(&bodies[1]).len(), 3);
    assert_eq!(known.lock().unwrap().len(), 3, "no fourth event id");
    let burned = burned_ids(home.path());
    assert_eq!(burned.len(), 3);
    assert_eq!(burned.iter().collect::<BTreeSet<_>>().len(), 3);
}

/// Append the first bytes of a record with no line end: the final record a
/// writer leaves when an append is cut short. The tail is written by the
/// test: an append is one write followed by a sync, and `SIGKILL` between two
/// of them leaves none.
fn cut_short_the_final_record(home: &Path, session: &str) {
    use std::io::Write as _;

    std::fs::OpenOptions::new()
        .append(true)
        .open(home.join(format!("sessions/{session}.ndjson")))
        .unwrap()
        .write_all(br#"{"seq":1,"timestamp":"2026-08-02T10:00:01.000Z","event":"crossing_med"#)
        .unwrap();
}

/// A writer killed inside an append leaves a final record cut short. The
/// session evidence contract keeps such a log unavailable to a strict reader
/// and repairs nothing, so a relay run skips that session, projects and
/// delivers the other session of the home, and fails at the end naming the
/// session and its file. Nothing of the damaged log is spooled or sent, by a
/// run of the whole home or by one scoped to it. The receiver is a loopback
/// server that accepts every batch.
#[test]
fn a_cut_short_final_record_is_skipped_by_name_and_the_other_session_is_delivered() {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut receiver = accepting_receiver(bodies.clone());
    let home = tempfile::tempdir().unwrap();
    for session in ["killed", "whole"] {
        write_session(
            home.path(),
            session,
            &[crossing_line(
                "crossing_mediated",
                session,
                &format!("https://host.example/{session}"),
                false,
            )],
        );
    }
    cut_short_the_final_record(home.path(), "killed");

    let options = |sessions: &[&str]| commonmeasure_relay::RelayOptions {
        receiver: Some(receiver.url()),
        sessions: sessions.iter().map(|id| (*id).to_owned()).collect(),
        ..Default::default()
    };
    let skipped_by = |scope: &[&str]| {
        let error = commonmeasure_relay::relay(home.path(), &options(scope))
            .expect_err("a log did not read");
        let text = error.to_string();
        assert!(text.contains("session killed was skipped"), "{text}");
        assert!(text.contains("killed.ndjson"), "{text}");
        let skipped = error
            .downcast::<commonmeasure_relay::UnreadableSessions>()
            .expect("the run completed for the sessions that read");
        let names: Vec<&str> = skipped
            .sessions
            .iter()
            .map(|session| session.session.as_str())
            .collect();
        assert_eq!(names, ["killed"]);
        skipped.report
    };

    // A forecast skips the log the same way and writes nothing.
    let error = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            dry_run: true,
            ..options(&[])
        },
    )
    .expect_err("a log did not read");
    let forecast = error
        .downcast::<commonmeasure_relay::UnreadableSessions>()
        .expect("the forecast completed for the session that read");
    assert_eq!(forecast.sessions[0].session, "killed");
    assert!(forecast.report.dry_run);
    assert_eq!(
        (
            forecast.report.sessions_read,
            forecast.report.events_delivered
        ),
        (1, 1)
    );
    assert!(
        bodies.lock().unwrap().is_empty(),
        "a forecast sends nothing"
    );
    assert!(
        !home.path().join("relay").exists(),
        "a forecast creates no relay state, the skipped-session account included"
    );

    let report = skipped_by(&[]);
    assert_eq!(
        (
            report.sessions_read,
            report.sessions_projected,
            report.events_delivered
        ),
        (1, 1, 1),
        "the damaged log is not counted as read"
    );
    let urls = |bodies: &[Value]| -> Vec<String> {
        bodies
            .iter()
            .flat_map(|body| body["events"].as_array().unwrap())
            .filter_map(|event| event["content_url"].as_str())
            .map(str::to_owned)
            .collect()
    };
    assert_eq!(
        urls(&bodies.lock().unwrap()),
        ["https://host.example/whole"],
        "the whole record before the cut-short one stays home too"
    );
    assert_eq!(burned_ids(home.path()).len(), 1);

    let report = skipped_by(&["killed"]);
    assert_eq!((report.sessions_read, report.events_enqueued), (0, 0));
    assert_eq!(bodies.lock().unwrap().len(), 1, "nothing more was sent");
    assert_eq!(burned_ids(home.path()).len(), 1);

    let report = commonmeasure_relay::relay(home.path(), &options(&["whole"])).unwrap();
    assert_eq!(
        (report.events_enqueued, report.events_delivered),
        (0, 0),
        "the first run delivered it"
    );
    receiver.stop();
}

/// One damaged log beside two sessions registered under an instance, whose
/// batches a reporting duty of that instance reads. The first run meets an
/// issuer that answers 503: the batch is spooled, and the delivery failure
/// names the skipped session as well. The second run, with the issuer
/// accepting, delivers the batch spooled before it and the batch it projects
/// itself, each with its `instance` members, and fails at the end naming the
/// damaged session. The issuer is a loopback double recording what the relay
/// posts, not a hub: this establishes what the edge sends and records while a
/// log is damaged, and nothing about a hub's duty read.
#[test]
fn a_damaged_log_does_not_hold_back_another_sessions_duty_bearing_batches() {
    use commonmeasure_relay::spool::Spool;
    use std::sync::atomic::{AtomicBool, Ordering};

    let accepting = Arc::new(AtomicBool::new(false));
    let bodies = Arc::new(Mutex::new(Vec::<Value>::new()));
    let mut issuer = {
        let (accepting, bodies) = (accepting.clone(), bodies.clone());
        commonmeasure_http::Server::bind("127.0.0.1:0")
            .unwrap()
            .spawn(move |request| {
                if !accepting.load(Ordering::SeqCst) {
                    return commonmeasure_http::Response::json(
                        503,
                        &json!({"detail": "unavailable"}).to_string(),
                    );
                }
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                let events = body["events"].as_array().map(Vec::len).unwrap_or(0);
                bodies.lock().unwrap().push(body);
                commonmeasure_http::Response::json(
                    201,
                    &json!({"status": "ok", "events_created": events}).to_string(),
                )
            })
            .unwrap()
    };
    let origin = issuer.url().trim_end_matches('/').to_owned();
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("relay.json"),
        json!({"receiver": format!("{origin}/api/v1/telemetry"), "api_key": "key"}).to_string(),
    )
    .unwrap();
    write_session(
        home.path(),
        "spooled",
        &[registered_line(
            "https://host.example/spooled",
            false,
            &origin,
            1,
        )],
    );
    write_session(
        home.path(),
        "killed",
        &[registered_line(
            "https://host.example/killed",
            false,
            &origin,
            1,
        )],
    );
    cut_short_the_final_record(home.path(), "killed");

    let start = chrono::Utc::now();
    let error = commonmeasure_relay::relay_with_clock(home.path(), &Default::default(), &|| start)
        .expect_err("the issuer answers 503");
    let failure = error
        .downcast_ref::<commonmeasure_relay::DeliveryFailure>()
        .expect("a delivery failure");
    assert_eq!(failure.unreadable_sessions.len(), 1);
    assert!(
        failure
            .delivery_text()
            .contains("session killed was skipped"),
        "{}",
        failure.delivery_text()
    );
    let due = Spool::read_only(home.path()).delivery_states().unwrap()[&0]
        .next_attempt_at
        .expect("a persisted deadline");

    write_session(
        home.path(),
        "later",
        &[registered_line(
            "https://host.example/later",
            false,
            &origin,
            1,
        )],
    );
    accepting.store(true, Ordering::SeqCst);
    let error = commonmeasure_relay::relay_with_clock(home.path(), &Default::default(), &|| due)
        .expect_err("the damaged log still fails the run");
    let skipped = error
        .downcast_ref::<commonmeasure_relay::UnreadableSessions>()
        .expect("every delivery succeeded");
    assert_eq!(skipped.sessions.len(), 1);
    assert_eq!(skipped.sessions[0].session, "killed");
    assert_eq!(
        (
            skipped.report.sessions_read,
            skipped.report.batches_delivered,
            skipped.report.events_delivered,
            skipped.report.batches_queued,
        ),
        (2, 2, 2, 0)
    );
    issuer.stop();

    let reference = json!({
        "issuer": origin,
        "id": "0b6f6c0e-5d0a-4a57-9f0e-3f1c2d4e5a6b",
        "revision": 1,
    });
    let mut delivered: Vec<(String, String, Value)> = bodies
        .lock()
        .unwrap()
        .iter()
        .flat_map(instance_members)
        .collect();
    delivered.sort_by(|a, b| a.1.cmp(&b.1));
    assert_eq!(
        delivered,
        vec![
            (
                "content_retrieved".to_owned(),
                "https://host.example/later".to_owned(),
                reference.clone()
            ),
            (
                "content_retrieved".to_owned(),
                "https://host.example/spooled".to_owned(),
                reference
            ),
        ],
        "both duty-bearing batches reached the issuer; nothing of the damaged log did"
    );
    assert_eq!(burned_ids(home.path()).len(), 2);
}

/// A loopback receiver that answers 503 until `accepting` is set, then
/// accepts every batch and keeps the bodies. A fault-injection double: it
/// establishes what the relay spools, holds and posts, not a hub's behaviour.
fn switchable_receiver(
    accepting: Arc<std::sync::atomic::AtomicBool>,
    bodies: Arc<Mutex<Vec<Value>>>,
) -> commonmeasure_http::ServerHandle {
    commonmeasure_http::Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(move |request| {
            if !accepting.load(std::sync::atomic::Ordering::SeqCst) {
                return commonmeasure_http::Response::text(503, "try later");
            }
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let events = body["events"].as_array().map(Vec::len).unwrap_or(0);
            bodies.lock().unwrap().push(body);
            commonmeasure_http::Response::json(
                201,
                &json!({"status": "ok", "events_created": events}).to_string(),
            )
        })
        .unwrap()
}

fn delivered_urls(bodies: &Mutex<Vec<Value>>) -> Vec<String> {
    bodies
        .lock()
        .unwrap()
        .iter()
        .flat_map(|body| body["events"].as_array().unwrap())
        .filter_map(|event| event["content_url"].as_str())
        .map(str::to_owned)
        .collect()
}

/// A batch is spooled from a session and the session's log is cut short
/// afterwards, in a home with a directory selection. The recheck before
/// delivery reads the origin log a second time, and consent cannot be
/// established from a log that does not read: the batch stays queued with a
/// hold that consumes no attempt, the session is named, and the batch behind
/// it in the spool is delivered. The durable egress account names the
/// session after the run. Once the log reads again the held batch leaves and
/// the account is cleared. The receiver is `switchable_receiver`.
#[test]
fn a_batch_whose_log_is_damaged_later_stays_queued_under_a_directory_selection() {
    use commonmeasure_relay::spool::{DeliveryStatus, Spool};
    use std::sync::atomic::{AtomicBool, Ordering};

    let accepting = Arc::new(AtomicBool::new(false));
    let bodies = Arc::new(Mutex::new(Vec::<Value>::new()));
    let mut receiver = switchable_receiver(accepting.clone(), bodies.clone());
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = temp.path().join("project");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    std::fs::write(
        home.join("policy.json"),
        json!({"policy_mode": "observe", "scopes": []}).to_string(),
    )
    .unwrap();
    commonmeasure_harness::directory::Registry::enrol(&home, &root, "Project", true).unwrap();
    let crossing = |session: &str| {
        crossing_in_cwd(
            "crossing_mediated",
            session,
            &format!("https://host.example/{session}"),
            false,
            root.to_str().unwrap(),
        )
    };
    let options = commonmeasure_relay::RelayOptions {
        receiver: Some(receiver.url()),
        ..Default::default()
    };

    write_session(&home, "pending", &[crossing("pending")]);
    let start = chrono::Utc::now();
    commonmeasure_relay::relay_with_clock(&home, &options, &|| start)
        .expect_err("the receiver answers 503");
    let due = Spool::read_only(&home).delivery_states().unwrap()[&0]
        .next_attempt_at
        .expect("a persisted deadline");

    cut_short_the_final_record(&home, "pending");
    write_session(&home, "later", &[crossing("later")]);
    accepting.store(true, Ordering::SeqCst);
    for tick in 0..2 {
        let at = due + chrono::Duration::hours(tick);
        let error = commonmeasure_relay::relay_with_clock(&home, &options, &|| at)
            .expect_err("the damaged log fails the run");
        let text = error.to_string();
        assert!(text.contains("session pending was skipped"), "{text}");
        assert!(text.contains("stays queued and unsent"), "{text}");
        let skipped = error
            .downcast::<commonmeasure_relay::UnreadableSessions>()
            .expect("the run went on past the held batch");
        assert_eq!(skipped.sessions.len(), 1, "named once, not once per read");
        assert_eq!(
            (
                skipped.sessions[0].session.as_str(),
                skipped.sessions[0].batches_held
            ),
            ("pending", 1)
        );
        assert_eq!(
            (
                skipped.report.sessions_read,
                skipped.report.batches_delivered,
                skipped.report.batches_queued,
            ),
            (1, 1 - tick as u64, 1)
        );
    }
    assert_eq!(
        delivered_urls(&bodies),
        ["https://host.example/later"],
        "the batch behind the held one left; the held one did not"
    );
    let held = Spool::read_only(&home).delivery_states().unwrap()[&0].clone();
    assert_eq!(held.status, DeliveryStatus::Queued);
    assert_eq!(held.attempts, 1, "a hold consumes no attempt");
    assert!(
        held.hold_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("does not read")),
        "{held:?}"
    );

    let egress = commonmeasure_relay::egress_report(&home);
    assert_eq!(egress["skipped_sessions"][0]["session"], "pending");
    assert_eq!(egress["skipped_sessions"][0]["batches_held"], 1);
    let status = commonmeasure_relay::state::egress_text(&egress);
    assert!(
        status.contains("The relay skipped session pending"),
        "{status}"
    );
    assert!(status.contains("pending.ndjson"), "{status}");

    // The log reads again, here because the test rewrites it whole. The next
    // claim clears the hold and the account drops the session.
    write_session(&home, "pending", &[crossing("pending")]);
    let at = due + chrono::Duration::hours(2);
    let report = commonmeasure_relay::relay_with_clock(&home, &options, &|| at).unwrap();
    assert_eq!((report.batches_delivered, report.batches_queued), (1, 0));
    assert_eq!(
        delivered_urls(&bodies),
        ["https://host.example/later", "https://host.example/pending"]
    );
    assert_eq!(
        commonmeasure_relay::egress_report(&home)["skipped_sessions"],
        json!([])
    );
    assert!(!home.join("relay/skipped-sessions.json").exists());
    receiver.stop();
}

/// The same sequence in a home without a directory selection: no recheck
/// reads the origin log, so the batch spooled before the damage is delivered
/// while the session is skipped and named. The run's last delivery succeeded,
/// so the egress account holds no error; it names the skipped session all the
/// same, and a later run scoped to another session does not clear an entry
/// for a log it never opened. The receiver is `switchable_receiver`.
#[test]
fn a_batch_whose_log_is_damaged_later_is_delivered_without_a_directory_selection() {
    use commonmeasure_relay::spool::Spool;
    use std::sync::atomic::{AtomicBool, Ordering};

    let accepting = Arc::new(AtomicBool::new(false));
    let bodies = Arc::new(Mutex::new(Vec::<Value>::new()));
    let mut receiver = switchable_receiver(accepting.clone(), bodies.clone());
    let home = tempfile::tempdir().unwrap();
    let options = |sessions: &[&str]| commonmeasure_relay::RelayOptions {
        receiver: Some(receiver.url()),
        sessions: sessions.iter().map(|id| (*id).to_owned()).collect(),
        ..Default::default()
    };
    let crossing = |session: &str| {
        crossing_line(
            "crossing_mediated",
            session,
            &format!("https://host.example/{session}"),
            false,
        )
    };

    write_session(home.path(), "pending", &[crossing("pending")]);
    let start = chrono::Utc::now();
    commonmeasure_relay::relay_with_clock(home.path(), &options(&[]), &|| start)
        .expect_err("the receiver answers 503");
    let due = Spool::read_only(home.path()).delivery_states().unwrap()[&0]
        .next_attempt_at
        .expect("a persisted deadline");

    cut_short_the_final_record(home.path(), "pending");
    accepting.store(true, Ordering::SeqCst);
    let error = commonmeasure_relay::relay_with_clock(home.path(), &options(&[]), &|| due)
        .expect_err("the damaged log fails the run");
    let skipped = error
        .downcast::<commonmeasure_relay::UnreadableSessions>()
        .expect("every delivery succeeded");
    assert_eq!(
        (
            skipped.sessions[0].session.as_str(),
            skipped.sessions[0].batches_held
        ),
        ("pending", 0)
    );
    assert_eq!(
        (
            skipped.report.sessions_read,
            skipped.report.batches_delivered,
            skipped.report.batches_queued,
        ),
        (0, 1, 0)
    );
    assert_eq!(delivered_urls(&bodies), ["https://host.example/pending"]);

    let egress = commonmeasure_relay::egress_report(home.path());
    assert_eq!(egress["last_error"], Value::Null);
    assert_eq!(egress["skipped_sessions"][0]["session"], "pending");
    assert!(
        egress["detail"]
            .as_str()
            .unwrap()
            .contains("The relay skipped session pending"),
        "{egress}"
    );

    write_session(home.path(), "whole", &[crossing("whole")]);
    commonmeasure_relay::relay_with_clock(home.path(), &options(&["whole"]), &|| due).unwrap();
    assert_eq!(
        commonmeasure_relay::egress_report(home.path())["skipped_sessions"][0]["session"],
        "pending",
        "a run that did not open the log does not clear its entry"
    );
    receiver.stop();
}

/// Two batches queued for the issuer, then delivered to another receiver that
/// accepts the first post and refuses the second. The accepted batch's events
/// are recorded delivered without their `instance` member, so the failure
/// carries the count and its text says so (EGR-07). The servers are loopback
/// receivers, not hubs.
#[test]
fn a_failed_run_still_reports_the_references_an_accepted_batch_lost() {
    let text =
        failure_after_an_accepted_batch(&["https://host.example/a", "https://host.example/b"]);
    assert!(
        text.contains(
            "warning: 2 events this run delivered were queued with an instance reference"
        ),
        "{text}"
    );
}

/// The same with one event in each batch, so the warning counts one event
/// and says so in the singular.
#[test]
fn a_failed_run_names_a_single_lost_reference_in_the_singular() {
    let text = failure_after_an_accepted_batch(&["https://host.example/a"]);
    assert!(
        text.contains("warning: 1 event this run delivered was queued with an instance reference"),
        "{text}"
    );
    assert!(!text.contains("1 events"), "{text}");
}

/// Relays two sessions, each recording `urls`, first to an issuer that
/// refuses, then to another receiver that accepts the first post and refuses
/// the second. Asserts the failure counts the accepted batch's withheld
/// references and names no "those events", and answers its text.
fn failure_after_an_accepted_batch(urls: &[&str]) -> String {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let mut issuer = commonmeasure_http::Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(|_| {
            commonmeasure_http::Response::json(503, &json!({"detail": "unavailable"}).to_string())
        })
        .unwrap();
    let issuer_origin = issuer.url().trim_end_matches('/').to_owned();
    let home = tempfile::tempdir().unwrap();
    for session in ["first", "second"] {
        let lines: Vec<String> = urls
            .iter()
            .map(|url| {
                let mut record: Value = serde_json::from_str(&registered_line(
                    &format!("{url}-{session}"),
                    // Ungrounded, so each line projects one event.
                    false,
                    &issuer_origin,
                    1,
                ))
                .unwrap();
                record["payload"]["session_id"] = json!(session);
                record.to_string()
            })
            .collect();
        write_session(home.path(), session, &lines);
    }
    std::fs::write(
        home.path().join("relay.json"),
        json!({"receiver": format!("{issuer_origin}/api/v1/telemetry"), "api_key": "key"})
            .to_string(),
    )
    .unwrap();
    commonmeasure_relay::relay(home.path(), &Default::default())
        .expect_err("the issuer answers 503");
    issuer.stop();

    let posts = Arc::new(AtomicUsize::new(0));
    let mut other = {
        let posts = posts.clone();
        commonmeasure_http::Server::bind("127.0.0.1:0")
            .unwrap()
            .spawn(move |request| {
                if posts.fetch_add(1, Ordering::SeqCst) > 0 {
                    return commonmeasure_http::Response::json(
                        503,
                        &json!({"detail": "unavailable"}).to_string(),
                    );
                }
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                let events = body["events"].as_array().map_or(0, Vec::len);
                commonmeasure_http::Response::json(
                    201,
                    &json!({"status": "ok", "events_created": events}).to_string(),
                )
            })
            .unwrap()
    };
    let error = relay_after_backoff(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(other.url()),
            ..Default::default()
        },
    )
    .expect_err("the second batch is refused");
    other.stop();
    assert_eq!(posts.load(Ordering::SeqCst), 2);
    let failure = error
        .downcast_ref::<commonmeasure_relay::DeliveryFailure>()
        .expect("a delivery failure");
    assert_eq!(
        failure.instance_references_withheld,
        urls.len() as u64,
        "the accepted batch's references were withheld"
    );
    let text = failure.delivery_text();
    assert!(
        text.contains("failed; undelivered batches remain"),
        "{text}"
    );
    assert!(!text.contains("of those events"), "{text}");
    assert_eq!(burned_ids(home.path()).len(), urls.len());
    text
}

/// Each request a [`key_recording_server`] saw: its target and its `X-API-Key`.
type SeenKeys = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// A loopback server that records the `X-API-Key` of every request beside its
/// target, accepts batches, and answers the enrolment status path for
/// `key_id`. It records what the relay sends; it is not a hub.
fn key_recording_server(seen: SeenKeys, key_id: &str) -> commonmeasure_http::ServerHandle {
    let key_id = key_id.to_owned();
    commonmeasure_http::Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(move |request| {
            seen.lock().unwrap().push((
                request.target.clone(),
                request.headers.get("x-api-key").map(str::to_owned),
            ));
            if request.method == "GET" {
                return commonmeasure_http::Response::json(
                    200,
                    &json!({"key_id": key_id}).to_string(),
                );
            }
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let events = body["events"].as_array().map_or(0, Vec::len);
            commonmeasure_http::Response::json(
                201,
                &json!({"status": "ok", "events_created": events}).to_string(),
            )
        })
        .unwrap()
}

/// EGR-05. The key in `relay.json` is the configured receiver's, on an
/// enrolled edge the hub's ingest key, and goes only to that receiver's
/// origin. A delivery re-targeted with `--receiver` carries `--api-key` or no
/// key, and a dry run reports no key.
///
/// Catches: the configured key posted to a receiver named on the command line.
#[test]
fn the_configured_api_key_goes_only_to_the_configured_receiver() {
    let configured_seen = Arc::new(Mutex::new(Vec::new()));
    let mut configured = key_recording_server(configured_seen.clone(), "unused");
    let other_seen = Arc::new(Mutex::new(Vec::new()));
    let mut other = key_recording_server(other_seen.clone(), "unused");
    let home_for = |session: &str| {
        let home = tempfile::tempdir().unwrap();
        write_session(
            home.path(),
            session,
            &[crossing_line(
                "crossing_mediated",
                session,
                "https://host.example/page",
                true,
            )],
        );
        std::fs::write(
            home.path().join("relay.json"),
            json!({
                "receiver": format!("{}/api/v1/telemetry", configured.url().trim_end_matches('/')),
                "api_key": "configured-key",
            })
            .to_string(),
        )
        .unwrap();
        home
    };
    let keys = |seen: &SeenKeys| -> Vec<Option<String>> {
        seen.lock().unwrap().drain(..).map(|(_, key)| key).collect()
    };

    let forecast = home_for("forecast");
    let report = commonmeasure_relay::relay(
        forecast.path(),
        &commonmeasure_relay::RelayOptions {
            dry_run: true,
            receiver: Some(other.url()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(report.events_delivered > 0);
    assert!(
        !format!("{report:?}").contains("configured-key"),
        "a dry run reports no key: {report:?}"
    );
    assert!(keys(&other_seen).is_empty() && keys(&configured_seen).is_empty());

    let retargeted = home_for("retargeted");
    let report = commonmeasure_relay::relay(
        retargeted.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(other.url()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(report.events_delivered > 0);
    assert_eq!(
        keys(&other_seen),
        vec![None],
        "a re-targeted delivery carries no X-API-Key"
    );

    let with_key = home_for("with-key");
    commonmeasure_relay::relay(
        with_key.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(other.url()),
            api_key: Some("other-key".to_owned()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(keys(&other_seen), vec![Some("other-key".to_owned())]);

    // Another path on the configured receiver's origin is the same receiver
    // for this rule, as it is for the instance reference.
    let same_origin = home_for("same-origin");
    commonmeasure_relay::relay(
        same_origin.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(format!(
                "{}/another/path",
                configured.url().trim_end_matches('/')
            )),
            ..Default::default()
        },
    )
    .unwrap();
    let unflagged = home_for("unflagged");
    commonmeasure_relay::relay(unflagged.path(), &Default::default()).unwrap();
    let configured_keys = keys(&configured_seen);
    assert!(
        !configured_keys.is_empty()
            && configured_keys
                .iter()
                .all(|key| key.as_deref() == Some("configured-key")),
        "the configured receiver gets the configured key: {configured_keys:?}"
    );
    assert!(keys(&other_seen).is_empty());
    configured.stop();
    other.stop();
}

/// The converse of EGR-05 on an enrolled edge: the standing check asks the
/// enrolled hub, which a re-targeted run does not deliver to. The hub is asked
/// under its own key from `relay.json`, and the key passed for the other
/// receiver goes to that receiver alone.
///
/// Catches: `--api-key`, given for another receiver, sent to the hub's
/// enrolment status path.
#[test]
fn the_standing_check_uses_the_hubs_key_whatever_receiver_is_named() {
    use commonmeasure_harness::enrolment::{
        EnrolledIdentity, EnrolledOrganization, EnrolmentRecord,
    };

    let hub_seen = Arc::new(Mutex::new(Vec::new()));
    let mut hub = key_recording_server(hub_seen.clone(), "key-1");
    let other_seen = Arc::new(Mutex::new(Vec::new()));
    let mut other = key_recording_server(other_seen.clone(), "unused");
    let hub_url = hub.url().trim_end_matches('/').to_owned();

    let home = tempfile::tempdir().unwrap();
    write_session(
        home.path(),
        "s1",
        &[crossing_line(
            "crossing_mediated",
            "s1",
            "https://host.example/page",
            true,
        )],
    );
    std::fs::write(
        home.path().join("relay.json"),
        json!({"receiver": format!("{hub_url}/api/v1/telemetry"), "api_key": "hub-key"})
            .to_string(),
    )
    .unwrap();
    EnrolmentRecord {
        hub: hub_url.clone(),
        organization: EnrolledOrganization {
            id: "org-1".to_owned(),
            name: "Org".to_owned(),
        },
        name: "laptop".to_owned(),
        key_id: "key-1".to_owned(),
        identity: EnrolledIdentity {
            origin: "https://hub.example".to_owned(),
            bot_page: "https://hub.example/bot".to_owned(),
            contact: None,
        },
        enrolled_at: "2026-09-18T00:00:00.000Z".to_owned(),
        revoked_at: None,
        revocation: None,
        revocation_learnt_at: None,
    }
    .store(home.path())
    .unwrap();

    let report = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(other.url()),
            api_key: Some("other-key".to_owned()),
            ..Default::default()
        },
    )
    .unwrap();
    hub.stop();
    other.stop();

    assert!(
        matches!(
            report.standing,
            Some(commonmeasure_relay::Standing::Enrolled { .. })
        ),
        "the hub answered under its own key: {:?}",
        report.standing
    );
    let hub_seen = hub_seen.lock().unwrap();
    assert!(!hub_seen.is_empty());
    for (target, key) in hub_seen.iter() {
        assert_eq!(key.as_deref(), Some("hub-key"), "{target}");
    }
    let other_seen = other_seen.lock().unwrap();
    assert!(!other_seen.is_empty());
    for (target, key) in other_seen.iter() {
        assert_eq!(key.as_deref(), Some("other-key"), "{target}");
    }
}

/// EGR-07. `relay.json` re-pointed by hand at another receiver, with that
/// receiver's key, on an edge still enrolled with its hub. The hosted
/// cadence's proof refresh and `disconnect` both ask the hub under the
/// `relay.json` key, and the key there is not the hub's, so neither sends it:
/// each reports that no key is configured for the hub and the hub receives
/// no request. The servers record what they are sent; they are not hubs.
///
/// Catches: the other receiver's key presented to the enrolled hub.
#[test]
fn a_key_held_for_another_receiver_is_not_sent_to_the_enrolled_hub() {
    use commonmeasure_harness::enrolment::{
        EnrolledIdentity, EnrolledOrganization, EnrolmentRecord,
    };

    let hub_seen = Arc::new(Mutex::new(Vec::new()));
    let mut hub = key_recording_server(hub_seen.clone(), "key-1");
    let hub_url = hub.url().trim_end_matches('/').to_owned();
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("relay.json"),
        json!({"receiver": "http://127.0.0.1:9/api/v1/telemetry", "api_key": "other-key"})
            .to_string(),
    )
    .unwrap();
    let record = EnrolmentRecord {
        hub: hub_url.clone(),
        organization: EnrolledOrganization {
            id: "org-1".to_owned(),
            name: "Org".to_owned(),
        },
        name: "laptop".to_owned(),
        key_id: "key-1".to_owned(),
        identity: EnrolledIdentity {
            origin: "https://hub.example".to_owned(),
            bot_page: "https://hub.example/bot".to_owned(),
            contact: None,
        },
        enrolled_at: "2026-09-18T00:00:00.000Z".to_owned(),
        revoked_at: None,
        revocation: None,
        revocation_learnt_at: None,
    };
    record.store(home.path()).unwrap();
    assert_eq!(record.hub_ingest_key(home.path()).unwrap(), None);

    let refresh = commonmeasure_relay::enrolment::refresh_directory_proof_if_due(
        home.path(),
        std::time::Duration::from_secs(5),
    )
    .expect("no listing is held, so a proof is due");
    assert!(
        matches!(
            &refresh.action,
            commonmeasure_relay::enrolment::ProofAction::NotSent(reason)
                if reason.contains("no ingest key is configured for the hub")
        ),
        "{refresh}"
    );

    let report = commonmeasure_relay::enrolment::disconnect(home.path()).unwrap();
    let refusal = report.revoked_at_hub.expect_err("the hub was not asked");
    assert!(
        refusal.contains("no ingest key is configured for the hub"),
        "{refusal}"
    );
    hub.stop();
    assert!(
        hub_seen.lock().unwrap().is_empty(),
        "the hub was sent nothing: {:?}",
        hub_seen.lock().unwrap()
    );

    // The file enrolment writes names the hub, and its key goes there.
    std::fs::write(
        home.path().join("relay.json"),
        json!({"receiver": format!("{hub_url}/api/v1/telemetry"), "api_key": "hub-key"})
            .to_string(),
    )
    .unwrap();
    assert_eq!(
        record.hub_ingest_key(home.path()).unwrap().as_deref(),
        Some("hub-key")
    );
}

/// The hash of the bytes an origin served stays in the operator's record.
/// What crosses the wire is `content_hash`, the hash of what entered
/// context, as before; `retrieved_hash` is the operator's evidence tying the
/// two and is nobody else's business.
#[test]
fn the_retrieved_hash_stays_off_the_wire() {
    let home = tempfile::tempdir().unwrap();
    let retrieved = "sha256:1111111111111111111111111111111111111111111111111111111111111111";
    let mut fetched: Value = serde_json::from_str(&crossing_line(
        "crossing_mediated",
        "s1",
        "https://host.example/page",
        true,
    ))
    .unwrap();
    fetched["payload"]["retrieved_hash"] = json!(retrieved);
    let content_hash = fetched["payload"]["content_hash"].clone();
    write_session(home.path(), "s1", &[fetched.to_string()]);

    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut receiver = accepting_receiver(bodies.clone());
    commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(receiver.url()),
            api_key: Some("key".to_owned()),
            runs: Vec::new(),
            sessions: Vec::new(),
            ..Default::default()
        },
    )
    .expect("relay");
    receiver.stop();

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1);
    let wire = bodies[0].to_string();
    assert!(
        !wire.contains(retrieved) && !wire.contains("retrieved_hash"),
        "the retrieved hash reached the receiver: {wire}"
    );
    let grounded: Vec<&Value> = bodies[0]["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["data"]["content_hash"].is_string())
        .collect();
    assert_eq!(grounded.len(), 1);
    assert_eq!(
        grounded[0]["data"]["content_hash"], content_hash,
        "data.content_hash is the hash of what entered context"
    );
}

/// Manifest discovery is its own record kind: the relay projects nothing
/// from it, so a probe of a publisher's well-known path can never reach a
/// receiver as a retrieval. The crossing beside it projects as before.
#[test]
fn a_manifest_record_is_never_projected() {
    let home = tempfile::tempdir().unwrap();
    let manifest_line = json!({
        "seq": 1,
        "timestamp": "2026-08-02T10:00:00.000Z",
        "event": "manifest_resolved",
        "payload": {
            "session_id": "s-manifest",
            "host": "host.example",
            "timestamp": "2026-08-02T10:00:00.000Z",
            "cache": "fetched",
            "fetched_at": "2026-08-02T10:00:00.000Z",
            "expires_at": "2026-08-02T11:00:00.000Z",
            "probes": [{"url": "https://host.example/.well-known/content-telemetry.json", "status": 200}],
            "outcome": "verified",
            "facts": {"schema_version": "1.0",
                      "id": "https://host.example/.well-known/content-telemetry.json",
                      "roles": ["content_owner"], "operator": "Host Example"},
        },
    })
    .to_string();
    write_session(
        home.path(),
        "s-manifest",
        &[
            manifest_line.clone(),
            crossing_line(
                "crossing_mediated",
                "s-manifest",
                "https://host.example/a",
                true,
            ),
        ],
    );
    write_session(home.path(), "s-manifest-only", &[manifest_line]);

    let bodies = Arc::new(Mutex::new(Vec::new()));
    let receiver = accepting_receiver(Arc::clone(&bodies));
    let report = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(receiver.url()),
            ..Default::default()
        },
    )
    .expect("relay");
    assert_eq!(report.sessions_read, 2);
    assert_eq!(report.sessions_projected, 1);
    assert_eq!(
        report.events_delivered, 2,
        "retrieval and grounding of the one crossing"
    );
    let delivered = bodies.lock().unwrap();
    assert_eq!(delivered.len(), 1);
    assert!(
        !delivered[0].to_string().contains("well-known"),
        "no manifest probe reaches the receiver"
    );
}

/// Terms naming institution identifiers require `access_context` on the
/// session. The event batch carries no session data, so such a session is
/// withheld and counted apart; a session under terms naming no identifier,
/// and one under no terms, are delivered as before.
#[test]
fn a_session_under_terms_naming_institution_identifiers_is_withheld_and_said_so() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("policy.json"),
        r#"{"scopes":[{"match":"/work/personal","engagement":"personal","allow_telemetry_egress":true,
            "terms":[
              {"host":"host.example","reference":"consortium-4471",
               "access_context":[{"scheme":"ror","value":"https://ror.org/013meh722"}]},
              {"host":"plain.example","reference":"agreement-7"}]}]}"#,
    )
    .unwrap();
    write_session(
        home.path(),
        "s-institution",
        &[crossing_line(
            "crossing_mediated",
            "s-institution",
            "https://host.example/a",
            true,
        )],
    );
    let plain = crossing_line(
        "crossing_mediated",
        "s-plain",
        "https://plain.example/a",
        true,
    )
    .replace(
        "\"host_name\":\"host.example\"",
        "\"host_name\":\"plain.example\"",
    );
    write_session(home.path(), "s-plain", &[plain]);
    let unnamed = crossing_line(
        "crossing_mediated",
        "s-none",
        "https://other.example/a",
        true,
    )
    .replace(
        "\"host_name\":\"host.example\"",
        "\"host_name\":\"other.example\"",
    );
    write_session(home.path(), "s-none", &[unnamed]);

    let bodies = Arc::new(Mutex::new(Vec::new()));
    let receiver = accepting_receiver(Arc::clone(&bodies));
    let report = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(receiver.url()),
            ..Default::default()
        },
    )
    .expect("relay");
    assert_eq!(report.sessions_read, 3);
    assert_eq!(report.sessions_withheld_access_context, 1);
    assert_eq!(report.sessions_withheld, 0);
    assert_eq!(report.sessions_projected, 2);
    let delivered = bodies.lock().unwrap();
    let text: String = delivered.iter().map(|body| body.to_string()).collect();
    assert!(
        !text.contains("host.example"),
        "the withheld session left nothing"
    );
    assert!(
        !text.contains("ror.org"),
        "no identifier crosses the wire in a batch"
    );
    assert!(text.contains("plain.example") && text.contains("other.example"));
    assert!(
        !home.path().join("relay/spool/outbound.ndjson").exists()
            || !std::fs::read_to_string(home.path().join("relay/spool/outbound.ndjson"))
                .unwrap()
                .contains("host.example"),
        "nothing of the withheld session was spooled"
    );
}

fn turn_line(event: &str, turn: Option<&str>, cwd: Option<&str>) -> String {
    json!({"event": event, "payload": {
        "timestamp": "2026-09-16T10:00:00Z", "host": "claude-code",
        "turn_id": turn, "privacy_level": "minimal",
        "detail": {"cwd": cwd, "transcript": "/private/transcript.jsonl"},
        "query_text": "private question", "response_text": "private answer",
        "query_intent": "private intent", "privacy_basis": "private basis",
    }})
    .to_string()
}

/// Exercises log reading, scope resolution, projection, spool serialisation,
/// HTTP delivery and deduplication. The loopback receiver records bytes; this
/// does not establish acceptance by Hub or an external conforming consumer.
#[test]
fn cleared_turn_boundaries_travel_at_minimal_privacy_and_keep_their_identity() {
    let home = tempfile::tempdir().unwrap();
    let mut undeclared: Value = serde_json::from_str(&turn_line(
        "turn_started",
        Some("undeclared"),
        Some("/work/personal"),
    ))
    .unwrap();
    undeclared["payload"]
        .as_object_mut()
        .unwrap()
        .remove("privacy_level");
    let mut higher = undeclared.clone();
    higher["payload"]["privacy_level"] = json!("full");
    higher["payload"]["turn_id"] = json!("higher");
    write_session(
        home.path(),
        "turns",
        &[
            turn_line("turn_started", Some("t1"), Some("/work/personal")),
            crossing_line(
                "crossing_observed",
                "turns",
                "https://publisher.example/page",
                true,
            ),
            turn_line("turn_completed", Some("t1"), Some("/work/personal")),
            turn_line("turn_started", Some("withheld"), Some("/work/client")),
            turn_line("turn_completed", Some("missing-cwd"), None),
            turn_line("turn_completed", None, Some("/work/personal")),
            undeclared.to_string(),
            higher.to_string(),
        ],
    );
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let receiver = accepting_receiver(Arc::clone(&bodies));
    let options = commonmeasure_relay::RelayOptions {
        receiver: Some(receiver.url()),
        ..Default::default()
    };
    let report = commonmeasure_relay::relay(home.path(), &options).unwrap();
    assert_eq!(report.events_delivered, 5);
    {
        let bodies = bodies.lock().unwrap();
        let events = bodies[0]["events"].as_array().unwrap();
        assert_eq!(
            events
                .iter()
                .map(|e| e["type"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "turn_started",
                "content_retrieved",
                "content_grounded",
                "turn_completed",
                "turn_completed"
            ]
        );
        for event in [&events[0], &events[3], &events[4]] {
            assert_eq!(event["turn"], json!({"privacy_level": "minimal"}));
            assert_eq!(event["timestamp"], "2026-09-16T10:00:00.000Z");
            assert!(event.get("content_url").is_none());
        }
        assert_eq!(events[0]["turn_id"], "t1");
        assert_eq!(events[3]["turn_id"], "t1");
        assert!(events[4].get("turn_id").is_none());
        let encoded = serde_json::to_string(&bodies[0]).unwrap();
        for private in [
            "/work/",
            "/private/",
            "private question",
            "private answer",
            "private intent",
            "private basis",
            "withheld",
            "missing-cwd",
            "undeclared",
            "higher",
        ] {
            assert!(
                !encoded.contains(private),
                "unexpected disclosure: {private}"
            );
        }
    }
    let again = commonmeasure_relay::relay(home.path(), &options).unwrap();
    assert_eq!(again.events_enqueued, 0);
    assert_eq!(again.events_delivered, 0);
    assert_eq!(bodies.lock().unwrap().len(), 1);
    // A policy change reveals only the newly cleared boundary; old ids survive.
    std::fs::write(
        home.path().join("policy.json"),
        r#"{"scopes":[{"match":"/work/","engagement":"work","allow_telemetry_egress":true}]}"#,
    )
    .unwrap();
    let widened = commonmeasure_relay::relay(home.path(), &options).unwrap();
    assert_eq!(widened.events_delivered, 1);
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies[1]["events"][0]["turn_id"], "withheld");
}

#[test]
fn selected_coverage_excludes_private_failed_refused_and_reconstructed_activity() {
    let home = tempfile::tempdir().unwrap();
    let mut failed: Value = serde_json::from_str(&crossing_line(
        "crossing_mediated",
        "selected",
        "https://failed.example/page",
        false,
    ))
    .unwrap();
    failed["payload"]["http_status"] = json!(403);
    let mut internal: Value = serde_json::from_str(&crossing_line(
        "crossing_mediated",
        "selected",
        "https://internal.example/page",
        true,
    ))
    .unwrap();
    internal["payload"]["internal"] = json!(true);
    let selected = crossing_line(
        "crossing_mediated",
        "selected",
        "https://selected.example/page",
        true,
    );
    write_session(
        home.path(),
        "selected",
        &[
            turn_line("turn_started", Some("t1"), Some("/work/personal")),
            selected,
            failed.to_string(),
            internal.to_string(),
            crossing_line(
                "crossing_refused",
                "selected",
                "https://refused.example/page",
                false,
            ),
            crossing_line(
                "crossing_reconstructed",
                "selected",
                "https://reconstructed.example/page",
                true,
            ),
            crossing_line(
                "crossing_observed",
                "selected",
                "http://127.0.0.1/private",
                true,
            ),
            crossing_in_cwd(
                "crossing_observed",
                "selected",
                "https://uncleared.example/page",
                true,
                "/work/client",
            ),
            turn_line("turn_completed", Some("t1"), Some("/work/personal")),
        ],
    );
    // A cleared boundary never makes an otherwise excluded session reportable.
    for (name, event, url) in [
        (
            "internal-only",
            "crossing_observed",
            "http://127.0.0.1/private",
        ),
        (
            "reconstructed-only",
            "crossing_reconstructed",
            "https://reconstructed.example/page",
        ),
        (
            "refused-only",
            "crossing_refused",
            "https://refused.example/page",
        ),
    ] {
        write_session(
            home.path(),
            name,
            &[
                turn_line("turn_started", Some(name), Some("/work/personal")),
                crossing_line(event, name, url, true),
            ],
        );
    }
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let receiver = accepting_receiver(Arc::clone(&bodies));
    let report = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(receiver.url()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(report.sessions_projected, 1);
    assert_eq!(report.events_delivered, 4);
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["refused"], 1);
    for event in bodies[0]["events"].as_array().unwrap() {
        assert_eq!(
            event["data"]["commonmeasure-projection"],
            json!({
                "conformance_level": "grounding",
                "coverage": {"mode": "selected", "terms_ref": "commonmeasure:telemetry-selection:v1"},
            })
        );
        if let Some(url) = event["content_url"].as_str() {
            assert_eq!(url, "https://selected.example/page");
        }
    }
}

#[test]
fn session_and_run_ingestion_counts_keep_the_recorded_basis_or_remain_unknown() {
    let home = tempfile::tempdir().unwrap();
    let cases = [
        (json!(13), json!("characters/4")),
        (json!(0), json!("whitespace-words")),
        (json!(9), Value::Null),
        (Value::Null, json!("characters/4")),
        (json!(4), json!("")),
    ];
    let mut lines = Vec::new();
    let mut runs = Vec::new();
    for (i, (count, basis)) in cases.iter().enumerate() {
        let mut crossing: Value = serde_json::from_str(&crossing_line(
            "crossing_mediated",
            "ingestion",
            &format!("https://publisher.example/{i}"),
            true,
        ))
        .unwrap();
        crossing["payload"]["estimated_tokens"] = count.clone();
        crossing["payload"]["token_basis"] = basis.clone();
        lines.push(crossing.to_string());
        let run = home.path().join(format!("run-{i}"));
        std::fs::create_dir(&run).unwrap();
        std::fs::write(run.join("summary.json"), json!({
            "run": {"id": uuid::Uuid::new_v4(), "started_at": "2026-09-16T10:00:00Z", "token_basis": basis},
            "plans": [{"id": "p", "sources": [{"admitted": true, "url": format!("https://publisher.example/{i}"), "retrieval_rank": 1, "tokens": count,
                "content_hash": "sha256:aa044138653177b57cacad53fa4ea2be3b2770e7177198820949acad706450de"}]}]
        }).to_string()).unwrap();
        runs.push(run);
    }
    write_session(home.path(), "ingestion", &lines);
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let receiver = accepting_receiver(Arc::clone(&bodies));
    let report = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(receiver.url()),
            runs,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(report.events_delivered, 20);
    let bodies = bodies.lock().unwrap();
    let mut grounded = 0;
    for event in bodies
        .iter()
        .flat_map(|body| body["events"].as_array().unwrap())
    {
        if event["type"] != "content_grounded" {
            assert!(event["data"].get("tokens_ingested").is_none());
            continue;
        }
        grounded += 1;
        let i = event["content_url"]
            .as_str()
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap()
            .parse::<usize>()
            .unwrap();
        if i < 2 {
            assert_eq!(event["data"]["tokens_ingested"], cases[i].0);
            assert_eq!(event["data"]["token_basis"], cases[i].1);
        } else {
            assert!(event["data"].get("tokens_ingested").is_none());
            assert!(event["data"].get("token_basis").is_none());
        }
        assert!(
            event["data"].get("chars_ingested").is_none(),
            "never reconstruct code points from a rounded token estimate"
        );
    }
    assert_eq!(grounded, 10);
}

fn relay_after_backoff(
    home: &std::path::Path,
    options: &commonmeasure_relay::RelayOptions,
) -> anyhow::Result<commonmeasure_relay::RelayReport> {
    commonmeasure_relay::relay_with_clock(home, options, &|| {
        chrono::Utc::now() + chrono::Duration::hours(1)
    })
}

/// Scripted responses exercise the real HTTP client and spool. This establishes
/// fixture-tested failure handling, not acceptance by Hub or another supplier.
#[test]
fn cadence_counts_claims_honours_backoff_and_requeues_dead_batches() {
    use commonmeasure_relay::spool::{DeliveryStatus, MAX_ATTEMPTS, Spool};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let home = tempfile::tempdir().unwrap();
    write_session(
        home.path(),
        "cadence",
        &[crossing_line(
            "crossing_observed",
            "cadence",
            "https://a.example/1",
            true,
        )],
    );
    let accept = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    let flag = accept.clone();
    let seen = calls.clone();
    let mut receiver = commonmeasure_http::Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(move |_| {
            let count = seen.fetch_add(1, Ordering::SeqCst);
            if flag.load(Ordering::SeqCst) {
                commonmeasure_http::Response::json(201, r#"{"status":"ok","events_created":2}"#)
            } else {
                commonmeasure_http::Response::text(
                    if count == 0 { 403 } else { 503 },
                    "not accepted",
                )
            }
        })
        .unwrap();
    let options = commonmeasure_relay::RelayOptions {
        receiver: Some(receiver.url()),
        ..Default::default()
    };
    let start = chrono::Utc::now();
    let mut now = start;
    for attempt in 1..=MAX_ATTEMPTS {
        let error =
            commonmeasure_relay::relay_with_clock(home.path(), &options, &|| now).unwrap_err();
        assert!(format!("{error:#}").contains("undelivered"));
        let state = Spool::read_only(home.path()).delivery_states().unwrap()[&0].clone();
        assert_eq!(state.attempts, attempt);
        assert_eq!(state.last_attempt_at, Some(now));
        assert_eq!(calls.load(Ordering::SeqCst), attempt as usize);
        assert!(state.last_error.as_ref().unwrap().contains("not accepted"));
        assert_eq!(
            commonmeasure_relay::egress_report(home.path())["delivered"],
            0
        );
        if attempt < MAX_ATTEMPTS {
            assert_eq!(state.status, DeliveryStatus::Queued);
            let due = state.next_attempt_at.unwrap();
            let expected = (60 * (1_i64 << (attempt - 1))).min(3600);
            assert_eq!((due - now).num_seconds(), expected);
            let before = due - chrono::Duration::milliseconds(1);
            let report =
                commonmeasure_relay::relay_with_clock(home.path(), &options, &|| before).unwrap();
            assert_eq!(report.batches_delivered, 0);
            assert_eq!(report.events_enqueued, 0);
            assert_eq!(calls.load(Ordering::SeqCst), attempt as usize);
            now = due;
        } else {
            assert_eq!(state.status, DeliveryStatus::Dead);
            assert!(state.next_attempt_at.is_none());
        }
    }
    let status = commonmeasure_relay::egress_report(home.path());
    assert_eq!(status["dead"], 1);
    assert_eq!(status["queued"], 0);
    assert_eq!(status["delivered_batches"], 0);
    assert!(status["last_error"].as_str().unwrap().contains("503"));
    let far_later = now + chrono::Duration::days(30);
    let report =
        commonmeasure_relay::relay_with_clock(home.path(), &options, &|| far_later).unwrap();
    assert_eq!(report.batches_dead, 1);
    assert_eq!(calls.load(Ordering::SeqCst), MAX_ATTEMPTS as usize);
    let queue_bytes = std::fs::read(home.path().join("relay/spool/outbound.ndjson")).unwrap();
    assert_eq!(
        Spool::open(home.path())
            .unwrap()
            .requeue(Some(0), far_later)
            .unwrap(),
        1
    );
    accept.store(true, Ordering::SeqCst);
    let report =
        commonmeasure_relay::relay_with_clock(home.path(), &options, &|| far_later).unwrap();
    assert_eq!(report.batches_delivered, 1);
    assert_eq!(report.events_enqueued, 0);
    let status = commonmeasure_relay::egress_report(home.path());
    assert_eq!(status["dead"], 0);
    assert_eq!(status["queued"], 0);
    assert_eq!(status["delivered_batches"], 1);
    assert_eq!(status["delivered"], 2);
    assert_eq!(
        queue_bytes,
        std::fs::read(home.path().join("relay/spool/outbound.ndjson")).unwrap()
    );
    receiver.stop();
}

#[test]
fn interrupted_claim_retries_on_schedule_and_final_interruption_becomes_dead() {
    use commonmeasure_relay::spool::{DeliveryStatus, MAX_ATTEMPTS, Spool, SpoolEntry};
    let home = tempfile::tempdir().unwrap();
    let now = chrono::Utc::now();
    let session = uuid::Uuid::new_v4();
    let event = uuid::Uuid::new_v4();
    {
        let spool = Spool::open(home.path()).unwrap();
        spool.enqueue(&SpoolEntry { origin: "claim fixture".into(), queued_at: Some(now), directory_selection: false,
            document: json!({"session_id":session,"agent_id":"commonmeasure","events":[{"id":event}]}) }).unwrap();
        assert!(spool.claim(0, now).unwrap());
        assert!(
            Spool::open(home.path()).is_err(),
            "a second writer cannot claim concurrently"
        );
        // Drop without HTTP or an outcome: equivalent durable state to a killed process.
    }
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut receiver = accepting_receiver(bodies.clone());
    let options = commonmeasure_relay::RelayOptions {
        receiver: Some(receiver.url()),
        ..Default::default()
    };
    let report = commonmeasure_relay::relay_with_clock(home.path(), &options, &|| now).unwrap();
    assert_eq!(report.batches_delivered, 0);
    assert!(bodies.lock().unwrap().is_empty());
    let later = now + chrono::Duration::seconds(60);
    let report = commonmeasure_relay::relay_with_clock(home.path(), &options, &|| later).unwrap();
    assert_eq!(report.batches_delivered, 1);
    assert_eq!(
        Spool::read_only(home.path()).delivery_states().unwrap()[&0].attempts,
        2
    );
    receiver.stop();

    let spool = Spool::open(home.path()).unwrap();
    let index = spool
        .enqueue(&SpoolEntry {
            origin: "interrupted final claim".into(),
            queued_at: Some(now),
            directory_selection: false,
            document: json!({"events":[]}),
        })
        .unwrap();
    let mut at = now;
    for _ in 0..MAX_ATTEMPTS {
        assert!(spool.claim(index, at).unwrap());
        at = spool.delivery_states().unwrap()[&index]
            .next_attempt_at
            .unwrap();
    }
    assert!(!spool.claim(index, at).unwrap());
    let state = spool.delivery_states().unwrap()[&index].clone();
    assert_eq!(state.status, DeliveryStatus::Dead);
    assert!(
        state
            .last_error
            .unwrap()
            .contains("acceptance has not been recorded")
    );
}

/// A receiver that answers 503 to a batch naming any of `refused` and
/// accepts every other, recording what it accepted.
fn refusing_receiver(
    refused: Arc<Mutex<Vec<&'static str>>>,
    bodies: Arc<Mutex<Vec<Value>>>,
) -> commonmeasure_http::ServerHandle {
    commonmeasure_http::Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(move |request| {
            let text = String::from_utf8_lossy(&request.body).into_owned();
            if refused
                .lock()
                .unwrap()
                .iter()
                .any(|host| text.contains(host))
            {
                return commonmeasure_http::Response::text(503, "refused");
            }
            bodies
                .lock()
                .unwrap()
                .push(serde_json::from_str(&text).unwrap());
            commonmeasure_http::Response::json(201, r#"{"status":"ok","events_created":1}"#)
        })
        .unwrap()
}

/// 0.3.4 and earlier acknowledged batches by an offset in `outbound.ack`,
/// which 0.3.5 and 0.4.0 read and never removed. A home that still carries
/// it is refused before anything is sent, and the error names the file and
/// the remedy. Where every session log survives, a spool moved aside is
/// rebuilt from the logs: each event the receiver has not accepted is sent
/// once, and no accepted event again.
#[test]
fn a_spool_carrying_outbound_ack_is_refused_and_a_spool_moved_aside_resends_no_accepted_event() {
    let home = tempfile::tempdir().unwrap();
    let sessions = ["accepted", "refused"];
    for session in sessions {
        write_session(
            home.path(),
            session,
            &[crossing_line(
                "crossing_observed",
                session,
                &format!("https://{session}.example/page"),
                true,
            )],
        );
    }
    let refused = Arc::new(Mutex::new(vec!["refused.example"]));
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut receiver = refusing_receiver(refused.clone(), bodies.clone());
    let options = commonmeasure_relay::RelayOptions {
        receiver: Some(receiver.url()),
        sessions: sessions
            .iter()
            .map(|session| (*session).to_owned())
            .collect(),
        ..Default::default()
    };
    commonmeasure_relay::relay(home.path(), &options).expect_err("one batch is refused");
    let urls = |bodies: &Mutex<Vec<Value>>| {
        let mut urls = delivered_urls(bodies);
        urls.dedup();
        urls
    };
    assert_eq!(urls(&bodies), ["https://accepted.example/page"]);

    let spool = home.path().join("relay/spool");
    std::fs::write(spool.join("outbound.ack"), "2\n").unwrap();
    refused.lock().unwrap().clear();
    bodies.lock().unwrap().clear();
    let error = format!(
        "{:#}",
        relay_after_backoff(home.path(), &options).expect_err("the spool is refused")
    );
    assert!(
        error.contains("outbound.ack was written by commonmeasure 0.3.4 or earlier")
            && error.contains("move relay/spool aside, keeping it and relay/delivered.idx"),
        "{error}"
    );
    assert!(bodies.lock().unwrap().is_empty(), "nothing is sent");
    let status = commonmeasure_relay::egress_report(home.path());
    assert!(
        status["unavailable"]
            .as_str()
            .is_some_and(|reason| reason.contains("outbound.ack")),
        "{status}"
    );
    assert!(status["queued"].is_null());

    std::fs::rename(&spool, home.path().join("relay/spool.0.3.4")).unwrap();
    let report = relay_after_backoff(home.path(), &options).expect("the remedy delivers");
    receiver.stop();
    assert_eq!(
        report.events_enqueued, 2,
        "the refused crossing's two events are projected again, and no accepted one"
    );
    assert_eq!(bodies.lock().unwrap().len(), 1);
    assert_eq!(urls(&bodies), ["https://refused.example/page"]);
}

/// A refused spool owing batches whose session logs partly survive, refused
/// for `outbound.ack` or for a line without an index. The relay sends
/// nothing, and the refusal and status name the remedy. Once the spool is
/// moved aside, a relay run without `--session` projects again from every
/// surviving log, not only the latest, and sends no event `delivered.idx`
/// records as accepted. The batch whose log is gone is not sent: its events
/// stay only in the saved spool.
#[test]
fn a_refused_spool_moved_aside_is_rebuilt_from_the_surviving_logs_alone() {
    for acknowledged in [true, false] {
        let home = tempfile::tempdir().unwrap();
        let sessions = ["accepted", "earlier", "rotated", "current"];
        for session in sessions {
            write_session(
                home.path(),
                session,
                &[crossing_line(
                    "crossing_observed",
                    session,
                    &format!("https://{session}.example/page"),
                    true,
                )],
            );
        }
        let refused = Arc::new(Mutex::new(vec![
            "earlier.example",
            "rotated.example",
            "current.example",
        ]));
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let mut receiver = refusing_receiver(refused.clone(), bodies.clone());
        let options = commonmeasure_relay::RelayOptions {
            receiver: Some(receiver.url()),
            ..Default::default()
        };
        commonmeasure_relay::relay(home.path(), &options).expect_err("three batches are refused");
        let mut urls = delivered_urls(&bodies);
        urls.dedup();
        assert_eq!(urls, ["https://accepted.example/page"]);

        std::fs::remove_file(home.path().join("sessions/rotated.ndjson")).unwrap();
        let spool = home.path().join("relay/spool");
        let queue_path = spool.join("outbound.ndjson");
        let expected = if acknowledged {
            std::fs::write(spool.join("outbound.ack"), "4\n").unwrap();
            "outbound.ack was written by commonmeasure 0.3.4 or earlier"
        } else {
            // The rotated session's batch as 0.3.4 wrote it: first, with no
            // index. Its recorded state names an index no line carries.
            let queue = std::fs::read_to_string(&queue_path).unwrap();
            let (mut old, current): (Vec<Value>, Vec<Value>) = queue
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .partition(|line| line.to_string().contains("rotated.example"));
            old[0].as_object_mut().unwrap().remove("index");
            let lines: String = old
                .iter()
                .chain(&current)
                .map(|line| format!("{line}\n"))
                .collect();
            std::fs::write(&queue_path, lines).unwrap();
            "spool line 0 (counted from zero) was written by commonmeasure 0.3.4 or earlier"
        };
        refused.lock().unwrap().clear();
        bodies.lock().unwrap().clear();
        let burned = burned_ids(home.path());
        let error = format!(
            "{:#}",
            relay_after_backoff(home.path(), &options).expect_err("the spool is refused")
        );
        assert!(
            error.contains(expected)
                && error.contains("move relay/spool aside, keeping it and relay/delivered.idx")
                && error.contains(
                    "an event whose session log or run input is gone stays in the saved spool"
                )
                && error.contains("(CHANGELOG.md, 0.4.1, Upgrading)")
                && !error.contains("0.4.0"),
            "{error}"
        );
        assert!(bodies.lock().unwrap().is_empty(), "nothing is sent");
        assert_eq!(burned_ids(home.path()), burned);

        let status = commonmeasure_relay::egress_report(home.path());
        assert!(status["queued"].is_null(), "{status}");
        assert!(
            status["unavailable"]
                .as_str()
                .is_some_and(|reason| reason.contains(expected)),
            "{status}"
        );
        // With `outbound.ack`, the three refused batches are indexed and
        // recorded as queued. Without it, the rotated batch's line has no
        // index, and its recorded state names a batch no indexed line
        // carries, so the count is incomplete.
        let counts = if acknowledged {
            json!({"outstanding": 3, "unknown": 0, "unindexed": 0, "incomplete": null})
        } else {
            json!({"outstanding": 2, "unknown": 0, "unindexed": 1,
                "incomplete": "the delivery metadata names 1 batch no indexed line carries"})
        };
        assert_eq!(status["refused_spool"], counts, "{status}");
        let text = commonmeasure_relay::state::egress_text(&status);
        assert!(
            text.contains("each is projected again only if its session log remains")
                && !text.contains("no indexed batch is outstanding"),
            "{text}"
        );

        let saved = home.path().join("relay/spool.saved");
        std::fs::rename(&spool, &saved).unwrap();
        let report = relay_after_backoff(home.path(), &options).expect("the relay runs");
        receiver.stop();
        assert_eq!(
            report.events_enqueued, 4,
            "the earlier and current sessions' events, from their surviving logs"
        );
        let mut urls = delivered_urls(&bodies);
        urls.sort();
        urls.dedup();
        assert_eq!(
            urls,
            [
                "https://current.example/page",
                "https://earlier.example/page"
            ]
        );
        let sent: Vec<String> = bodies
            .lock()
            .unwrap()
            .iter()
            .flat_map(|body| body["events"].as_array().unwrap().clone())
            .map(|event| event["id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(sent.len(), 4);
        assert!(
            sent.iter().all(|id| !burned.contains(id)),
            "no event recorded as accepted is sent again"
        );
        let delivered = burned_ids(home.path());
        assert!(burned.iter().all(|id| delivered.contains(id)));
        assert_eq!(delivered.len(), burned.len() + 4);
        let status = commonmeasure_relay::egress_report(home.path());
        assert_eq!(status["queued"], 0, "the rotated batch is not counted");
        assert!(status["refused_spool"].is_null());
        assert!(
            std::fs::read_to_string(saved.join("outbound.ndjson"))
                .unwrap()
                .contains("rotated.example"),
            "its events stay only in the saved spool"
        );
    }
}

/// A batch 0.3.4 or earlier queued, whose line a prune by 0.3.5 or 0.4.0
/// rewrote with an index and `queued_at: null`, may carry consent
/// provenance the rewrite filled in. It is held with a reason that names
/// it, and never sent; a batch queued since is delivered beside it.
#[test]
fn a_batch_queued_by_0_3_4_or_earlier_is_held_and_never_sent() {
    use commonmeasure_relay::spool::{DeliveryStatus, PRE_0_3_5_HOLD, Spool};
    let home = tempfile::tempdir().unwrap();
    write_session(
        home.path(),
        "early",
        &[crossing_line(
            "crossing_observed",
            "early",
            "https://early.example/page",
            true,
        )],
    );
    let refused = Arc::new(Mutex::new(vec!["early.example"]));
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut receiver = refusing_receiver(refused.clone(), bodies.clone());
    let options = commonmeasure_relay::RelayOptions {
        receiver: Some(receiver.url()),
        ..Default::default()
    };
    commonmeasure_relay::relay(home.path(), &options).expect_err("the batch is refused");

    let queue_path = home.path().join("relay/spool/outbound.ndjson");
    let mut line: Value =
        serde_json::from_str(std::fs::read_to_string(&queue_path).unwrap().trim()).unwrap();
    assert_eq!(line["index"], 0);
    line["queued_at"] = Value::Null;
    std::fs::write(&queue_path, format!("{line}\n")).unwrap();
    write_session(
        home.path(),
        "later",
        &[crossing_line(
            "crossing_observed",
            "later",
            "https://later.example/page",
            true,
        )],
    );
    refused.lock().unwrap().clear();
    for _ in 0..2 {
        relay_after_backoff(home.path(), &options).expect("the later batch is delivered");
    }
    receiver.stop();
    assert_eq!(bodies.lock().unwrap().len(), 1);
    let mut urls = delivered_urls(&bodies);
    urls.dedup();
    assert_eq!(urls, ["https://later.example/page"]);
    let states = Spool::read_only(home.path()).delivery_states().unwrap();
    assert_eq!(states[&0].status, DeliveryStatus::Queued);
    assert_eq!(states[&0].hold_reason.as_deref(), Some(PRE_0_3_5_HOLD));
    assert_eq!(states[&0].attempts, 1, "a hold consumes no attempt");
    let status = commonmeasure_relay::egress_report(home.path());
    assert_eq!(status["held"], 1);
    assert_eq!(status["hold_reason"], PRE_0_3_5_HOLD);
    assert!(status["oldest_queued_age_seconds"].is_null());
}

#[test]
fn later_acceptance_never_acknowledges_an_earlier_failed_batch() {
    use commonmeasure_relay::spool::{DeliveryStatus, Spool};
    let home = tempfile::tempdir().unwrap();
    for session in ["a-fail", "b-accept"] {
        write_session(
            home.path(),
            session,
            &[crossing_line(
                "crossing_observed",
                session,
                &format!("https://{session}.example/page"),
                true,
            )],
        );
    }
    let mut receiver = commonmeasure_http::Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(|request| {
            if String::from_utf8_lossy(&request.body).contains("a-fail.example") {
                commonmeasure_http::Response::text(503, "refused")
            } else {
                commonmeasure_http::Response::json(201, r#"{"status":"ok","events_created":2}"#)
            }
        })
        .unwrap();
    let result = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(receiver.url()),
            sessions: vec!["a-fail".into(), "b-accept".into()],
            ..Default::default()
        },
    );
    assert!(result.is_err());
    let states = Spool::read_only(home.path()).delivery_states().unwrap();
    assert_eq!(states[&0].status, DeliveryStatus::Queued);
    assert_eq!(states[&1].status, DeliveryStatus::Delivered);
    let status = commonmeasure_relay::egress_report(home.path());
    assert_eq!(status["queued"], 1);
    assert_eq!(status["delivered_batches"], 1);
    assert_eq!(status["delivered"], 2);
    assert!(status["last_error"].as_str().unwrap().contains("503"));
    receiver.stop();
}

#[test]
fn a_silent_receiver_times_out_and_keeps_the_claim_undelivered() {
    use commonmeasure_relay::spool::Spool;
    use std::net::TcpListener;
    let home = tempfile::tempdir().unwrap();
    write_session(
        home.path(),
        "timeout",
        &[crossing_line(
            "crossing_observed",
            "timeout",
            "https://a.example/page",
            true,
        )],
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (done, wait) = std::sync::mpsc::channel::<()>();
    let server = std::thread::spawn(move || {
        let (_stream, _) = listener.accept().unwrap();
        let _ = wait.recv_timeout(std::time::Duration::from_secs(40));
    });
    let error = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(url),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("did not answer within"),
        "{error:#}"
    );
    done.send(()).unwrap();
    server.join().unwrap();
    let state = Spool::read_only(home.path()).delivery_states().unwrap()[&0].clone();
    assert_eq!(state.attempts, 1);
    assert!(state.next_attempt_at.is_some());
    assert_eq!(
        commonmeasure_relay::egress_report(home.path())["delivered"],
        0
    );
}

#[test]
fn an_unclaimed_batch_has_no_invented_retry_deadline() {
    use commonmeasure_relay::spool::{Spool, SpoolEntry};
    let home = tempfile::tempdir().unwrap();
    let spool = Spool::open(home.path()).unwrap();
    spool
        .enqueue(&SpoolEntry {
            origin: "unclaimed fixture".into(),
            queued_at: Some(chrono::Utc::now()),
            directory_selection: false,
            document: json!({"events":[{"id":uuid::Uuid::new_v4()}]}),
        })
        .unwrap();
    let status = commonmeasure_relay::egress_report(home.path());
    assert_eq!(status["queued"], 1);
    assert_eq!(status["due_now"], 1);
    assert!(status["next_attempt_at"].is_null());
    assert!(status["oldest_queued_age_seconds"].is_number());
    assert!(commonmeasure_relay::state::egress_text(&status).contains("due now"));
}

/// Every file under `home` with its bytes, so a forecast can be shown to
/// have written nothing.
fn home_files(home: &Path) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    let mut files = std::collections::BTreeMap::new();
    let mut pending = vec![home.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    files
}

fn posted_by_type(bodies: &Mutex<Vec<Value>>) -> std::collections::BTreeMap<String, u64> {
    let mut counts = std::collections::BTreeMap::new();
    for body in bodies.lock().unwrap().iter() {
        for event in body["events"].as_array().unwrap() {
            *counts
                .entry(event["type"].as_str().unwrap().to_owned())
                .or_default() += 1;
        }
    }
    counts
}

fn turn_pair(turn: &str) -> [String; 2] {
    [
        turn_line("turn_started", Some(turn), Some("/work/personal")),
        turn_line("turn_completed", Some(turn), Some("/work/personal")),
    ]
}

/// Catches (EGR-25): a forecast that counts less than a real run sends. The
/// home holds the three kinds of work a real run delivers: a batch an earlier
/// run spooled and failed to deliver, turn boundaries appended to a session
/// whose content events were delivered before, and a session never relayed.
/// The dry run and the real run read the same home under the same clock; the
/// forecast's per-type counts must equal the real run's and what the loopback
/// receiver was posted, and the forecast must leave every file as it was.
#[test]
fn a_dry_run_forecasts_per_type_exactly_what_the_real_run_then_sends() {
    let home = tempfile::tempdir().unwrap();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut accepting = accepting_receiver(Arc::clone(&bodies));

    // Delivered before: content and one turn.
    let delivered_url = "https://delivered.example/page";
    let mut earlier = turn_pair("t1").to_vec();
    earlier.insert(
        1,
        crossing_line("crossing_observed", "earlier", delivered_url, true),
    );
    write_session(home.path(), "earlier", &earlier);
    let first = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(accepting.url()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(first.events_delivered, 4);

    // Spooled and undelivered: a receiver that answers 503.
    let failing_server = commonmeasure_http::Server::bind("127.0.0.1:0").unwrap();
    let mut failing = failing_server
        .spawn(|_| commonmeasure_http::Response::json(503, r#"{"detail":"unavailable"}"#))
        .unwrap();
    let mut queued = turn_pair("q1").to_vec();
    queued.insert(
        1,
        crossing_line(
            "crossing_observed",
            "queued",
            "https://queued.example/a",
            true,
        ),
    );
    queued.extend(turn_pair("q2"));
    write_session(home.path(), "queued", &queued);
    commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(failing.url()),
            ..Default::default()
        },
    )
    .expect_err("the 503 leaves the batch spooled");
    failing.stop();

    // New since: turns after the delivered session's content, and a session
    // never relayed.
    earlier.extend(turn_pair("t2"));
    earlier.extend(turn_pair("t3"));
    write_session(home.path(), "earlier", &earlier);
    let mut fresh = turn_pair("f1").to_vec();
    fresh.insert(
        1,
        crossing_line(
            "crossing_observed",
            "fresh",
            "https://fresh.example/b",
            false,
        ),
    );
    write_session(home.path(), "fresh", &fresh);

    bodies.lock().unwrap().clear();
    let options = commonmeasure_relay::RelayOptions {
        receiver: Some(accepting.url()),
        ..Default::default()
    };
    let before = home_files(home.path());
    let forecast = relay_after_backoff(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            dry_run: true,
            ..options
        },
    )
    .unwrap();
    assert_eq!(home_files(home.path()), before, "a forecast writes nothing");
    assert!(
        bodies.lock().unwrap().is_empty(),
        "a forecast sends nothing"
    );
    let options = commonmeasure_relay::RelayOptions {
        receiver: Some(accepting.url()),
        ..Default::default()
    };
    let real = relay_after_backoff(home.path(), &options).unwrap();
    accepting.stop();

    let expected: std::collections::BTreeMap<String, u64> = [
        ("content_grounded", 1),
        ("content_retrieved", 2),
        ("turn_completed", 5),
        ("turn_started", 5),
    ]
    .into_iter()
    .map(|(kind, count)| (kind.to_owned(), count))
    .collect();
    assert_eq!(real.events_by_type, expected);
    assert_eq!(posted_by_type(&bodies), expected);
    assert_eq!(forecast.events_by_type, real.events_by_type);
    assert_eq!(forecast.events_delivered, real.events_delivered);
    assert_eq!(forecast.batches_delivered, real.batches_delivered);
    assert_eq!(forecast.events_enqueued, real.events_enqueued);
    assert_eq!(
        forecast.events_enqueued, 7,
        "the newly spooled count leaves out the batch an earlier run queued"
    );
    assert_eq!(real.events_delivered, 13);
}
