//! The relay boundary and durability proofs, offline.
//!
//! The counterparty for the offline tests is a real HTTP server on loopback
//! (`commonmeasure_http::Server`), receiving the real bytes the relay posts; the proof
//! against the real oa-server implementation is the ignored live test in
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

/// A receiver whose answer is a 2xx but not the telemetry receiver's own: the
/// shape a proxy, a captive portal or a wrong path on the right host returns.
fn misanswering_receiver(
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
    let run = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../demo/output/latest");

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

    // A receiver that answers 2xx without the receiver's own acceptance, so the
    // batch spools and delivery fails.
    let mut refusing = misanswering_receiver(200, "text/html", "<html>hello</html>");
    let error = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(refusing.url()),
            ..Default::default()
        },
    )
    .expect_err("a foreign 2xx is not an acceptance");
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
    let report = commonmeasure_relay::relay(
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
        commonmeasure_relay::project::project_session("s-mixed", &records, &[], &|_| true).batches;

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
    let batches_a =
        commonmeasure_relay::project::project_session("s-owner-a", &parse(&owner_a), &[], &|_| {
            true
        })
        .batches;
    let batches_b =
        commonmeasure_relay::project::project_session("s-owner-b", &parse(&owner_b), &[], &|_| {
            true
        })
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
    // recorded. Removing the ack file reopens exactly that window.
    std::fs::remove_file(home.path().join("relay/spool/outbound.ack")).unwrap();
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

/// A 2xx is not an acceptance. The captive-portal shape — 200 with an HTML
/// body — must fail the delivery, keep the spool and burn no ids, because a
/// burned id can never be sent to the true receiver; the second half of the
/// test is that the true receiver still gets the batch afterwards.
#[test]
fn a_two_hundred_with_an_html_body_fails_and_leaves_the_batch_deliverable() {
    let home = tempfile::tempdir().unwrap();
    write_session(
        home.path(),
        "s-portal",
        &[crossing_line(
            "crossing_observed",
            "s-portal",
            "https://a.example/1",
            true,
        )],
    );
    let mut portal = misanswering_receiver(
        200,
        "text/html; charset=utf-8",
        "<html><body>Sign in to continue</body></html>",
    );

    let error = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(portal.url()),
            ..Default::default()
        },
    )
    .expect_err("a 2xx that is not the receiver's answer must fail the delivery");
    let message = format!("{error:#}");
    assert!(
        message.contains("not as a telemetry receiver"),
        "the failure must name what was wrong with the answer, got: {message}"
    );
    assert!(message.contains("remain spooled"), "got: {message}");
    portal.stop();

    assert!(
        burned_ids(home.path()).is_empty(),
        "no id may be recorded as delivered on an answer that proves nothing"
    );
    let egress = commonmeasure_relay::egress_report(home.path());
    assert_eq!(egress["delivered"], 0);
    assert_eq!(egress["pending"], 2, "the spool keeps the batch intact");
    assert!(
        egress["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("Last attempt failed"),
        "the console must report the failure, got {}",
        egress["detail"]
    );

    // The point of not burning the ids: the true receiver still gets them.
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let mut real = accepting_receiver(bodies.clone());
    let recovered = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(real.url()),
            ..Default::default()
        },
    )
    .expect("the true receiver must still be reachable");
    assert_eq!(recovered.events_delivered, 2);
    assert_eq!(recovered.events_new_at_receiver, 2);
    assert_eq!(bodies.lock().unwrap().len(), 1, "one real delivery");
    real.stop();
}

/// The other half of the same rule: a well-formed JSON body that does not
/// carry the receiver's acceptance is no acceptance either.
#[test]
fn a_two_hundred_and_one_with_an_empty_json_body_is_a_delivery_failure() {
    let home = tempfile::tempdir().unwrap();
    write_session(
        home.path(),
        "s-empty-json",
        &[crossing_line(
            "crossing_observed",
            "s-empty-json",
            "https://a.example/1",
            true,
        )],
    );
    let mut receiver = misanswering_receiver(201, "application/json", "{}");

    let error = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(receiver.url()),
            ..Default::default()
        },
    )
    .expect_err("201 without the receiver's acceptance must fail");
    assert!(format!("{error:#}").contains("not as a telemetry receiver"));
    receiver.stop();

    assert!(burned_ids(home.path()).is_empty());
    let egress = commonmeasure_relay::egress_report(home.path());
    assert_eq!(egress["delivered"], 0);
    assert_eq!(egress["pending"], 2);
}

/// `status: "ok"` without a count says nothing about what was recorded, and a
/// count reported as 0 in that case is the report the operator would act on.
#[test]
fn an_acceptance_without_an_events_created_count_is_a_delivery_failure() {
    let home = tempfile::tempdir().unwrap();
    write_session(
        home.path(),
        "s-no-count",
        &[crossing_line(
            "crossing_observed",
            "s-no-count",
            "https://a.example/1",
            true,
        )],
    );
    let mut receiver = misanswering_receiver(200, "application/json", "{\"status\":\"ok\"}");

    let error = commonmeasure_relay::relay(
        home.path(),
        &commonmeasure_relay::RelayOptions {
            receiver: Some(receiver.url()),
            ..Default::default()
        },
    )
    .expect_err("an acceptance with no count must fail");
    assert!(format!("{error:#}").contains("no events_created count"));
    receiver.stop();

    assert!(burned_ids(home.path()).is_empty());
    assert_eq!(
        commonmeasure_relay::egress_report(home.path())["pending"],
        2
    );
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
    let recovered = commonmeasure_relay::relay(
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
