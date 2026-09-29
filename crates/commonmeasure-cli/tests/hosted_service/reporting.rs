//! Reporting from a hosted home: the relay projects what the hook recorded
//! under a locally enrolled directory, and only under a current signed
//! owner approval. A hosted session has a working directory only where
//! `hosted-service.json` declares one.
use super::*;
use commonmeasure_harness::directory::Registry;

fn configure(home: &Path) {
    std::fs::write(
        home.join("hosted-service.json"),
        json!({
            "listen": "127.0.0.1:0", "origin": ORIGIN, "hosts": ["m365-copilot"],
            "interval_seconds": 300,
        })
        .to_string(),
    )
    .expect("configuration");
}

/// This is a synthetic observed-source fixture through the real hook and
/// relay, not evidence of a live Copilot acquisition or answer use.
#[test]
fn an_enrolled_directory_reports_only_with_a_signed_approval_and_stops_after_revocation() {
    let hub = Hub::start();
    let home = tempfile::tempdir().expect("home");
    let root = tempfile::tempdir().expect("root");
    hub.enrol_managed(home.path());
    let project =
        Registry::enrol(home.path(), root.path(), "Hosted test", true).expect("selection");
    configure(home.path());

    let cwd = root
        .path()
        .canonicalize()
        .expect("canonical root")
        .to_str()
        .expect("UTF-8")
        .to_owned();
    let observe = |session: &str| observe(home.path(), &cwd, session, FIXTURE_URL);
    let approve = |revision: u64, approvals: Value| approve(&hub, home.path(), revision, approvals);
    let relay = || relay_ok(home.path());
    observe("declared-scope-fixture-1");
    relay();
    assert!(
        hub.batches.lock().expect("batches").is_empty(),
        "no owner approval"
    );
    approve(
        1,
        json!([{"project_id":project.id,"binding":project.binding}]),
    );
    relay();
    let delivered = hub.batches.lock().expect("batches").len();
    assert!(
        delivered > 0,
        "a valid approval must permit eligible evidence"
    );
    approve(2, json!([]));
    observe("declared-scope-fixture-2");
    relay();
    assert_eq!(
        hub.batches.lock().expect("batches").len(),
        delivered,
        "revoked approval withholds new evidence"
    );
}

const FIXTURE_URL: &str = "https://www.example.com/fixture";

/// One synthetic crossing of `url` through the real hook, in `cwd`.
fn observe(home: &Path, cwd: &str, session: &str, url: &str) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["hook", "post-tool-use"])
        .env("COMMONMEASURE_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("hook");
    writeln!(
        child.stdin.as_mut().expect("stdin"),
        "{}",
        json!({
            "session_id": session, "cwd": cwd,
            "hook_event_name": "PostToolUse", "tool_name": "WebFetch",
            "tool_input": {"url": url},
            "tool_response": {"result":"Synthetic fixture text for reporting permission checks."}
        })
    )
    .expect("fixture input");
    drop(child.stdin.take());
    assert!(child.wait().expect("hook finishes").success());
}

/// A reporting-approval snapshot signed by the hub's policy key, valid for
/// `lifetime` from now.
fn signed_approvals(
    hub: &Hub,
    home: &Path,
    revision: u64,
    approvals: Value,
    lifetime: chrono::Duration,
) -> Value {
    let edge = commonmeasure_harness::EnrolmentRecord::load(home)
        .expect("enrolment")
        .expect("enrolled");
    let payload = json!({"schema":"commonmeasure-reporting-approvals/v1",
        "organisation": ORGANISATION, "edge_key_id": edge.key_id,
        "revision": revision, "approvals": approvals});
    let now = chrono::Utc::now();
    let mut snapshot = json!({"payload":payload,"digest":canonical_digest(&payload),
        "key_id":SIGNER_KEY_ID,"issued_at":now,"expires_at":now+lifetime});
    snapshot["signature"] = json!(encode_hex(
        hub.policy_signer
            .sign(canonical_json(&snapshot).as_bytes())
            .as_ref()
    ));
    snapshot
}

/// A reporting-approval snapshot accepted as `enrol --sync` accepts one.
/// Unless the test sets the hub's approvals, the loopback hub answers the
/// approvals route `404`, so the relay's own refresh fails and the accepted
/// snapshot stands.
fn approve(hub: &Hub, home: &Path, revision: u64, approvals: Value) {
    let snapshot = signed_approvals(hub, home, revision, approvals, chrono::Duration::hours(1));
    commonmeasure_harness::directory::accept(home, &snapshot).expect("valid signed snapshot");
}

fn relay_ok(home: &Path) {
    let (ok, stdout, stderr) = run(home, &["relay"]);
    assert!(ok, "{stdout}\n{stderr}");
}

/// Fault injection while no relay runs: move every persisted retry deadline
/// into the past. The CLI has no clock override.
fn make_retry_due(home: &Path) {
    let path = home.join("relay/spool/outbound.delivery.json");
    let mut states: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("delivery state")).expect("JSON");
    for state in states.as_object_mut().expect("states").values_mut() {
        state["next_attempt_at"] = json!("2000-01-01T00:00:00Z");
    }
    std::fs::write(path, serde_json::to_vec(&states).expect("JSON")).expect("write");
}

/// Every event id the hub received, with how many times it arrived, and the
/// content URLs among them.
fn received(hub: &Hub) -> (std::collections::BTreeMap<String, usize>, Vec<String>) {
    let mut ids = std::collections::BTreeMap::new();
    let mut urls = Vec::new();
    for batch in hub.batches.lock().expect("batches").iter() {
        for event in batch["events"].as_array().expect("events") {
            *ids.entry(event["id"].as_str().expect("id").to_owned())
                .or_default() += 1;
            if let Some(url) = event["content_url"].as_str() {
                urls.push(url.to_owned());
            }
        }
    }
    (ids, urls)
}

/// EGR-48, through the real hook and relay against a loopback hub. A batch
/// spooled while approved and due after the approval is revoked is held, not
/// dropped: nothing is sent, and the spool records it held. Evidence recorded
/// while approved and first relayed after revocation is not projected. After
/// re-approval the next relay sends both, each event once, and a further
/// relay sends nothing.
#[test]
fn a_revoked_approval_holds_evidence_and_re_approval_sends_it_once() {
    const QUEUED: &str = "https://www.example.com/queued";
    const RECORDED: &str = "https://www.example.com/recorded";
    let hub = Hub::start();
    let home = tempfile::tempdir().expect("home");
    let root = tempfile::tempdir().expect("root");
    hub.enrol_managed(home.path());
    let project = Registry::enrol(home.path(), root.path(), "Held test", true).expect("selection");
    let cwd = root.path().canonicalize().expect("canonical root");
    let cwd = cwd.to_str().expect("UTF-8");
    let approved = json!([{"project_id": project.id, "binding": project.binding}]);

    approve(&hub, home.path(), 1, approved.clone());
    observe(home.path(), cwd, "held-queued", QUEUED);
    hub.delivery_outage.store(true, Ordering::SeqCst);
    assert!(!run(home.path(), &["relay"]).0, "the outage fails delivery");
    hub.delivery_outage.store(false, Ordering::SeqCst);
    observe(home.path(), cwd, "held-recorded", RECORDED);

    approve(&hub, home.path(), 2, json!([]));
    make_retry_due(home.path());
    relay_ok(home.path());
    assert!(
        hub.batches.lock().expect("batches").is_empty(),
        "a revoked approval sends nothing"
    );
    let egress = commonmeasure_relay::egress_report(home.path());
    assert_eq!(egress["held"], 1, "the queued batch is held: {egress}");
    assert!(egress["hold_reason"].is_string(), "{egress}");
    assert_eq!(egress["delivered"], 0);

    approve(&hub, home.path(), 3, approved);
    relay_ok(home.path());
    let (ids, urls) = received(&hub);
    assert!(urls.iter().any(|url| url == QUEUED), "{urls:?}");
    assert!(urls.iter().any(|url| url == RECORDED), "{urls:?}");
    assert!(
        ids.values().all(|&times| times == 1),
        "each event is sent once: {ids:?}"
    );
    let egress = commonmeasure_relay::egress_report(home.path());
    assert_eq!(egress["held"], 0, "{egress}");
    assert_eq!(egress["pending"], 0, "{egress}");

    let batches = hub.batches.lock().expect("batches").len();
    relay_ok(home.path());
    assert_eq!(
        hub.batches.lock().expect("batches").len(),
        batches,
        "a further relay sends nothing"
    );
}

/// The sequence of the hub's real-edge directory test, where EGR-48 was
/// seen. A failed delivery sets the batch's retry deadline; a relay before
/// that deadline neither rechecks nor sends the batch, under revocation or
/// after re-approval. The batch is queued throughout and is sent once when
/// the deadline passes.
#[test]
fn a_batch_in_retry_backoff_at_re_approval_is_sent_when_due() {
    let hub = Hub::start();
    let home = tempfile::tempdir().expect("home");
    let root = tempfile::tempdir().expect("root");
    hub.enrol_managed(home.path());
    let project =
        Registry::enrol(home.path(), root.path(), "Backoff test", true).expect("selection");
    let cwd = root.path().canonicalize().expect("canonical root");
    let approved = json!([{"project_id": project.id, "binding": project.binding}]);

    approve(&hub, home.path(), 1, approved.clone());
    observe(
        home.path(),
        cwd.to_str().expect("UTF-8"),
        "backoff",
        FIXTURE_URL,
    );
    hub.delivery_outage.store(true, Ordering::SeqCst);
    assert!(!run(home.path(), &["relay"]).0, "the outage fails delivery");
    hub.delivery_outage.store(false, Ordering::SeqCst);

    approve(&hub, home.path(), 2, json!([]));
    relay_ok(home.path());
    approve(&hub, home.path(), 3, approved);
    relay_ok(home.path());
    assert!(
        hub.batches.lock().expect("batches").is_empty(),
        "a batch before its retry deadline is not sent"
    );
    let egress = commonmeasure_relay::egress_report(home.path());
    assert_eq!(egress["held"], 0, "not rechecked, so not held: {egress}");
    assert!(egress["next_attempt_at"].is_string(), "{egress}");
    assert!(
        egress["pending"].as_u64().is_some_and(|n| n > 0),
        "{egress}"
    );

    make_retry_due(home.path());
    relay_ok(home.path());
    let (ids, urls) = received(&hub);
    assert!(urls.iter().any(|url| url == FIXTURE_URL), "{urls:?}");
    assert!(ids.values().all(|&times| times == 1), "{ids:?}");
    let batches = hub.batches.lock().expect("batches").len();
    relay_ok(home.path());
    assert_eq!(hub.batches.lock().expect("batches").len(), batches);
}

/// EGR-49, through the real hook and relay against a loopback hub. An
/// approval that expires withholds the evidence; once the hub renews it, the
/// relay's own refresh restores clearance and that same relay sends it, with
/// no `enrol --sync` between.
#[test]
fn an_approval_renewed_after_expiry_clears_evidence_on_the_next_relay() {
    let hub = Hub::start();
    let home = tempfile::tempdir().expect("home");
    let root = tempfile::tempdir().expect("root");
    hub.enrol_managed(home.path());
    let project =
        Registry::enrol(home.path(), root.path(), "Renewal test", true).expect("selection");
    let cwd = root.path().canonicalize().expect("canonical root");
    let approved = json!([{"project_id": project.id, "binding": project.binding}]);

    let lifetime = Duration::from_secs(2);
    let short = signed_approvals(
        &hub,
        home.path(),
        1,
        approved.clone(),
        chrono::Duration::from_std(lifetime).expect("duration"),
    );
    let expires = Instant::now() + lifetime;
    commonmeasure_harness::directory::accept(home.path(), &short).expect("valid signed snapshot");
    observe(
        home.path(),
        cwd.to_str().expect("UTF-8"),
        "renewal",
        FIXTURE_URL,
    );
    std::thread::sleep(
        expires.saturating_duration_since(Instant::now()) + Duration::from_millis(200),
    );

    relay_ok(home.path());
    assert!(
        hub.batches.lock().expect("batches").is_empty(),
        "an expired approval sends nothing"
    );

    *hub.approvals.lock().expect("approvals") = Some(signed_approvals(
        &hub,
        home.path(),
        2,
        approved,
        chrono::Duration::hours(1),
    ));
    relay_ok(home.path());
    let (ids, urls) = received(&hub);
    assert!(urls.iter().any(|url| url == FIXTURE_URL), "{urls:?}");
    assert!(ids.values().all(|&times| times == 1), "{ids:?}");
}

/// A licence whose reporting demand this runtime can meet: grounding
/// conformance through the Content Telemetry profile.
const REPORTING_LICENCE: &str = r#"<rsl xmlns="https://rslstandard.org/rsl">
  <content url="/"><license>
    <permits type="usage">ai-input</permits>
    <reporting type="telemetry" profile="https://contenttelemetry.org/profiles/spur">
      <![CDATA[{"conformance_level": "grounding"}]]>
    </reporting>
  </license></content></rsl>"#;

/// A publisher whose licence demands grounding telemetry reporting, on
/// loopback.
fn reporting_site() -> ServerHandle {
    Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(|request| match request.target.as_str() {
            "/robots.txt" => {
                Response::text(200, "License: /license.xml\nUser-agent: *\nAllow: /\n")
            }
            "/license.xml" => Response::new(200, REPORTING_LICENCE.as_bytes().to_vec()),
            "/.well-known/content-telemetry.json" => Response::text(404, "no manifest"),
            _ => Response::text(200, "the reported article"),
        })
        .expect("spawn")
}

/// `site`'s URL under a public name. The relay never projects a crossing of
/// a loopback address, so a reporting demand there is never met; the debug
/// binary resolves this name to loopback (`COMMONMEASURE_TEST_HOSTS`, set
/// by [`StdioServer`]), and the hub's policy lets the fetch reach it.
fn public(site: &ServerHandle) -> String {
    site.url().replace("127.0.0.1", "publisher.test")
}

/// A Claude Code MCP server on stdio, as the host starts it: the real
/// binary in the session's directory, initialised before it is asked
/// anything.
struct StdioServer {
    child: Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    log: std::path::PathBuf,
    /// From the spawn to the answer to `initialize`, which the server
    /// gives only once the session has opened.
    started_in: Duration,
}

impl StdioServer {
    fn start(home: &Path, cwd: &Path, session: &str) -> Self {
        let spawned = Instant::now();
        let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
            .args(["mcp", "--host", "claude-code", "--session", session])
            .env("COMMONMEASURE_HOME", home)
            .env("HOME", home)
            .env("COMMONMEASURE_TEST_HOSTS", "publisher.test=127.0.0.1")
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the server starts");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let mut server = Self {
            child,
            stdin,
            stdout,
            log: home.join(format!("sessions/{session}.ndjson")),
            started_in: Duration::ZERO,
        };
        server.ask(json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
                          "params": {"protocolVersion": "2025-11-25", "capabilities": {},
                                     "clientInfo": {"name": "claude-code", "version": "1"}}}));
        server.started_in = spawned.elapsed();
        server
    }

    fn ask(&mut self, request: Value) -> Value {
        writeln!(self.stdin, "{request}").expect("write request");
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("a response");
        serde_json::from_str(&line).expect("one JSON object per line")
    }

    fn fetch(&mut self, id: u64, url: &str) -> Value {
        self.ask(json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
                        "params": {"name": "context_fetch", "arguments": {"url": url}}}))
    }

    /// The reporting ruling on the session's `index`th crossing.
    fn reporting(&self, index: usize) -> Value {
        commonmeasure_harness::SessionLog::read(&self.log)
            .expect("session log")
            .into_iter()
            .filter(|record| {
                record["event"]
                    .as_str()
                    .is_some_and(|event| event.starts_with("crossing_"))
            })
            .nth(index)
            .expect("the crossing")["payload"]["declarations"]["reporting"]
            .clone()
    }

    fn stop(mut self) {
        drop(self.stdin);
        assert!(self.child.wait().expect("the server exits").success());
    }
}

/// Owner decision, 27 September 2026 (consent before an obligated
/// crossing), on a managed, directory-selected home: the operator's
/// reporting consent, not the hub's signed approval, decides a reporting
/// demand. A Claude Code MCP server started while an approval cleared its
/// directory refuses the demand until the operator agrees, the approval
/// notwithstanding; once agreed, the demand is met, and it stays met after
/// the approval expires with the server still running, recorded as met in a
/// scope that clears nothing. The approval lives three seconds; the hub
/// serves no renewal.
#[test]
fn the_reporting_consent_not_the_approval_decides_a_demand_on_a_selected_home() {
    let hub = Hub::start();
    let home = tempfile::tempdir().expect("home");
    let root = tempfile::tempdir().expect("root");
    hub.enrol_managed(home.path());
    let project =
        Registry::enrol(home.path(), root.path(), "Expiry test", true).expect("selection");
    // A relay run applies the hub's policy, under which alone an approval
    // is honoured.
    relay_ok(home.path());
    let lifetime = Duration::from_secs(3);
    let short = signed_approvals(
        &hub,
        home.path(),
        1,
        json!([{"project_id": project.id, "binding": project.binding}]),
        chrono::Duration::from_std(lifetime).expect("duration"),
    );
    let expires = Instant::now() + lifetime;
    commonmeasure_harness::directory::accept(home.path(), &short).expect("valid signed snapshot");
    let cwd = root.path().canonicalize().expect("canonical root");
    assert!(
        commonmeasure_harness::policy::SessionPolicy::load(home.path(), cwd.to_str())
            .expect("policy")
            .allows_telemetry_egress(),
        "the approval clears the directory"
    );

    let site = reporting_site();
    let mut server = StdioServer::start(home.path(), &cwd, "expiry");
    let unconsented = server.fetch(1, &format!("{}/unconsented", public(&site)));
    assert_eq!(unconsented["result"]["isError"], true, "{unconsented}");
    let ruling = server.reporting(0);
    assert_eq!(ruling["met"], false);
    assert_eq!(ruling["telemetry_egress_cleared"], true);
    assert_eq!(ruling["consent_needed"], true);

    agree(home.path());
    let approved = server.fetch(2, &format!("{}/approved", public(&site)));
    assert_eq!(approved["result"]["isError"], false, "{approved}");
    assert_eq!(server.reporting(1)["met"], true, "{}", server.reporting(1));

    std::thread::sleep(
        expires.saturating_duration_since(Instant::now()) + Duration::from_millis(200),
    );
    let expired = server.fetch(3, &format!("{}/expired", public(&site)));
    assert_eq!(expired["result"]["isError"], false, "{expired}");
    let ruling = server.reporting(2);
    assert_eq!(ruling["met"], true, "{ruling}");
    assert_eq!(ruling["telemetry_egress_cleared"], false);
    assert_eq!(ruling["consent"]["state"], "agreed");
    server.stop();
}

/// Record the operator's reporting consent in `home`.
fn agree(home: &Path) {
    commonmeasure_harness::consent::record(
        home,
        commonmeasure_harness::consent::Answer::Agreed,
        chrono::Utc::now(),
    )
    .expect("consent");
}

/// A managed, directory-selected home with the hub's policy applied, whose
/// only approval snapshot has expired: accepted while current, it lives two
/// seconds. Returns the approval that clears the root, for the hub to
/// renew.
fn expired_approvals(hub: &Hub, home: &Path, root: &Path) -> Value {
    let project = Registry::enrol(home, root, "Renewal test", true).expect("selection");
    relay_ok(home);
    let lifetime = Duration::from_secs(2);
    let approved = json!([{"project_id": project.id, "binding": project.binding}]);
    let short = signed_approvals(
        hub,
        home,
        1,
        approved.clone(),
        chrono::Duration::from_std(lifetime).expect("duration"),
    );
    let expires = Instant::now() + lifetime;
    commonmeasure_harness::directory::accept(home, &short).expect("valid signed snapshot");
    std::thread::sleep(
        expires.saturating_duration_since(Instant::now()) + Duration::from_millis(200),
    );
    approved
}

/// EGR-179. A home with no background relay whose approval snapshot expired
/// clears its directory again in the next session: the session start renews
/// the snapshot from the hub, and the demand ruled under the operator's
/// consent records the scope cleared. A second start, under a snapshot early
/// in its life, asks the hub nothing.
#[test]
fn a_session_started_after_the_approvals_expired_renews_them_and_meets_a_demand() {
    let hub = Hub::start();
    let home = tempfile::tempdir().expect("home");
    let root = tempfile::tempdir().expect("root");
    hub.enrol_managed(home.path());
    let approved = expired_approvals(&hub, home.path(), root.path());
    *hub.approvals.lock().expect("approvals") = Some(signed_approvals(
        &hub,
        home.path(),
        2,
        approved,
        chrono::Duration::hours(1),
    ));
    let cwd = root.path().canonicalize().expect("canonical root");
    let site = reporting_site();
    agree(home.path());

    let asked = hub.approval_requests.load(Ordering::SeqCst);
    let mut server = StdioServer::start(home.path(), &cwd, "renewed");
    assert_eq!(hub.approval_requests.load(Ordering::SeqCst), asked + 1);
    let fetched = server.fetch(1, &format!("{}/renewed", public(&site)));
    assert_eq!(fetched["result"]["isError"], false, "{fetched}");
    assert_eq!(server.reporting(0)["met"], true, "{}", server.reporting(0));
    assert_eq!(server.reporting(0)["telemetry_egress_cleared"], true);
    server.stop();

    let again = StdioServer::start(home.path(), &cwd, "current");
    assert_eq!(
        hub.approval_requests.load(Ordering::SeqCst),
        asked + 1,
        "a snapshot in the first half of its life is not renewed at a start"
    );
    again.stop();
}

/// EGR-179. With the hub answering neither the approvals nor the policy
/// route, the renewal and the policy refresh together cost the start one
/// session-start budget, not one each, and the session opens under the
/// expired snapshot: its scope clears no egress, which the ruling records,
/// and without the operator's consent the demand is refused. Breaks where
/// the renewal waits a budget of its own after the policy refresh has spent
/// the shared one.
#[test]
fn a_hub_that_does_not_answer_delays_the_start_by_the_budget_and_the_scope_stays_uncleared() {
    let hub = Hub::start();
    let home = tempfile::tempdir().expect("home");
    let root = tempfile::tempdir().expect("root");
    hub.enrol_managed(home.path());
    let approved = expired_approvals(&hub, home.path(), root.path());
    *hub.approvals.lock().expect("approvals") = Some(signed_approvals(
        &hub,
        home.path(),
        2,
        approved,
        chrono::Duration::hours(1),
    ));
    hub.approvals_stalled.store(true, Ordering::SeqCst);
    hub.policy_stalled.store(true, Ordering::SeqCst);
    let before = std::fs::read(home.path().join("reporting-approvals.json")).expect("snapshot");
    let cwd = root.path().canonicalize().expect("canonical root");
    let site = reporting_site();

    let asked = hub.approval_requests.load(Ordering::SeqCst);
    let mut server = StdioServer::start(home.path(), &cwd, "stalled");
    assert_eq!(hub.approval_requests.load(Ordering::SeqCst), asked + 1);
    let budget = commonmeasure_harness::managed::SESSION_START_BUDGET;
    assert!(
        server.started_in < budget + Duration::from_secs(2),
        "the start took {:?}",
        server.started_in
    );
    let refused = server.fetch(1, &format!("{}/stalled", public(&site)));
    assert_eq!(refused["result"]["isError"], true, "{refused}");
    let ruling = server.reporting(0);
    assert_eq!(ruling["met"], false);
    assert_eq!(ruling["telemetry_egress_cleared"], false);
    assert_eq!(ruling["consent_needed"], true);
    assert_eq!(
        std::fs::read(home.path().join("reporting-approvals.json")).expect("snapshot"),
        before,
        "the snapshot is kept"
    );
    server.stop();
}

/// The licence `https://commonmeasure.ai/license.xml` serves, byte for byte
/// as the site publishes it (`site` `ai/public/license.xml`, 29 September
/// 2026): usage permitted, telemetry reporting at grounding level demanded.
const COMMONMEASURE_LICENCE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- The terms under which this site's content may be used by machines, in
     Really Simple Licensing (RSL) 1.0. Named from robots.txt and from the
     Link header on every response. Search, AI input and AI indexing are
     permitted without payment; AI training is not permitted. A client using
     the content under this licence reports that use as Content Telemetry
     events at grounding level to the endpoint below (RSL 1.0 §3.12), the
     same endpoint /.well-known/content-telemetry.json declares. -->
<rsl xmlns="https://rslstandard.org/rsl">
  <content url="/">
    <license>
      <permits type="usage">search ai-input ai-index</permits>
      <prohibits type="usage">ai-train</prohibits>
      <payment type="free"/>
      <reporting type="telemetry" profile="https://contenttelemetry.org/profiles/spur" endpoint="https://telemetry.openattribution.org/events"><![CDATA[
        { "conformance_level": "grounding", "privacy_level": "minimal" }
      ]]></reporting>
    </license>
  </content>
</rsl>
"#;

/// `commonmeasure.ai` on loopback: its `robots.txt`, which names the
/// licence, the licence, and a page. The debug binary resolves the name to
/// this server (`COMMONMEASURE_TEST_HOSTS`, [`hosted_service`]).
fn commonmeasure_site() -> ServerHandle {
    Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(|request| match request.target.as_str() {
            "/robots.txt" => Response::text(
                200,
                "License: /license.xml\n\nUser-agent: *\n\
                 Content-Signal: search=yes, ai-input=yes, ai-train=no\nAllow: /\n",
            ),
            "/license.xml" => Response::new(200, COMMONMEASURE_LICENCE.as_bytes().to_vec()),
            "/.well-known/content-telemetry.json" => Response::text(404, "no manifest"),
            _ => Response::text(200, "Getting started with Common Measure."),
        })
        .expect("spawn")
}

/// A loopback site that serves one page and demands nothing.
fn undemanding_site() -> ServerHandle {
    Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(|request| match request.target.as_str() {
            "/robots.txt" => Response::text(200, "User-agent: *\nAllow: /\n"),
            "/.well-known/content-telemetry.json" => Response::text(404, "no manifest"),
            _ => Response::text(200, "an undemanding page"),
        })
        .expect("spawn")
}

/// The page of the live reproduction on EDG-88, on `site`.
fn getting_started(site: &ServerHandle) -> String {
    format!(
        "{}/docs/getting-started/",
        site.url().replace("127.0.0.1", "commonmeasure.ai")
    )
}

/// The hosted service on `home`, with the names the tests fetch resolved
/// to loopback.
fn hosted_service(home: &Path) -> Service {
    let mut command = service_command(home);
    command.env(
        "COMMONMEASURE_TEST_HOSTS",
        "commonmeasure.ai=127.0.0.1,publisher.test=127.0.0.1,institution.test=127.0.0.1",
    );
    Service::start_with(command, Stdio::null())
}

/// `hosted-service.json` serving `m365-copilot`, with `session_directory`
/// when one is given.
fn configure_scoped(home: &Path, directory: Option<&Path>) {
    let mut config = json!({
        "listen": "127.0.0.1:0", "origin": ORIGIN, "hosts": ["m365-copilot"],
        "interval_seconds": 300,
    });
    if let Some(directory) = directory {
        config["session_directory"] = json!(directory);
    }
    std::fs::write(home.join("hosted-service.json"), config.to_string()).expect("configuration");
}

/// The pilot's policy (`ops-word-edge-046`): `observe` with no constraints
/// at the top level, and a scope for `directory` under the engagement
/// `m365-word-public-test` that clears egress and admits `commonmeasure.ai`
/// alone, strictly. `extra` is merged into the top level.
fn pilot_policy(directory: &Path, extra: Value) -> Value {
    let mut policy = json!({
        "policy_mode": "observe",
        "scopes": [{
            "match": directory.to_str().expect("UTF-8"),
            "engagement": "m365-word-public-test",
            "allow_telemetry_egress": true,
            "policy_mode": "strict",
            "constraints": [
                {"kind": "allowed_source_host", "host": "commonmeasure.ai"},
                {"kind": "denied_source_host", "host": "example.com"}
            ]
        }]
    });
    for (key, value) in extra.as_object().expect("an object") {
        policy[key] = value.clone();
    }
    policy
}

/// The records of hosted session `session` in `home`.
fn session_records(home: &Path, session: &str) -> Vec<Value> {
    commonmeasure_harness::SessionLog::read(&home.join(format!("sessions/{session}.ndjson")))
        .expect("session log")
}

/// The `(type, content_url)` of every event the hub received.
fn delivered_events(hub: &Hub) -> Vec<(String, String)> {
    hub.batches
        .lock()
        .expect("batches")
        .iter()
        .flat_map(|body| body["events"].as_array().cloned().unwrap_or_default())
        .map(|event| {
            (
                event["type"].as_str().unwrap_or_default().to_owned(),
                event["content_url"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

/// EDG-88, the live reproduction of 29 September 2026 05:42 UTC through a
/// loopback fixture. A Microsoft 365 Copilot session on a hosted service
/// whose `hosted-service.json` declares the pilot's scope directory runs
/// under that scope, not the top-level policy: `context_status` names the
/// directory, the scope and its engagement, and the scope's strict host list
/// refuses a host the top level would admit. Under the operator's reporting
/// consent, the `commonmeasure.ai` page whose licence demands reporting is
/// admitted with the demand met and the scope's clearance recorded; the log
/// records the directory's basis; and the relay delivers the page's
/// retrieval and grounding to the loopback receiver.
#[test]
fn a_hosted_session_under_a_declared_scope_is_governed_by_it_and_its_report_is_delivered() {
    let hub = Hub::start();
    let home = tempfile::tempdir().expect("home");
    let scope = tempfile::tempdir().expect("scope");
    let directory = scope.path().canonicalize().expect("canonical");
    hub.enrol_managed(home.path());
    hub.publish(1, pilot_policy(&directory, json!({})));
    configure_scoped(home.path(), Some(&directory));
    agree(home.path());
    let site = commonmeasure_site();
    let service = hosted_service(home.path());
    let auth = hub.bearer("user-1", "m365-copilot");
    let session = service.open_session("m365-copilot", &auth);

    let status = payload(&service.call(
        "m365-copilot",
        &auth,
        &session,
        1,
        "context_status",
        json!({}),
    ));
    assert_eq!(
        status["cwd"],
        directory.to_str().expect("UTF-8"),
        "{status}"
    );
    assert_eq!(
        status["policy"]["scope"],
        directory.to_str().expect("UTF-8")
    );
    assert_eq!(status["policy"]["mode"], "strict");
    assert_eq!(
        status["policy"]["governing_engagement"],
        "m365-word-public-test"
    );

    let page = getting_started(&site);
    let fetched = service.call(
        "m365-copilot",
        &auth,
        &session,
        2,
        "context_fetch",
        json!({"url": page}),
    );
    assert_eq!(
        body(&fetched)["result"]["isError"],
        false,
        "{}",
        text(&fetched)
    );
    let other = format!(
        "{}/article",
        site.url().replace("127.0.0.1", "publisher.test")
    );
    let refused = service.call(
        "m365-copilot",
        &auth,
        &session,
        3,
        "context_fetch",
        json!({"url": other}),
    );
    assert_eq!(
        body(&refused)["result"]["isError"],
        true,
        "{}",
        text(&refused)
    );

    let records = session_records(home.path(), &session);
    let scope_record = records
        .iter()
        .find(|record| record["event"] == "hosted_scope")
        .expect("the scope's basis is recorded");
    assert_eq!(scope_record["payload"]["basis"], "service_configuration");
    assert_eq!(
        scope_record["payload"]["directory"],
        directory.to_str().expect("UTF-8")
    );
    let crossings: Vec<&Value> = records
        .iter()
        .filter(|record| {
            record["event"]
                .as_str()
                .is_some_and(|event| event.starts_with("crossing_"))
        })
        .collect();
    assert_eq!(crossings[0]["event"], "crossing_mediated");
    assert_eq!(
        crossings[0]["payload"]["cwd"],
        directory.to_str().expect("UTF-8")
    );
    let ruling = &crossings[0]["payload"]["declarations"]["reporting"];
    assert_eq!(ruling["met"], true, "{ruling}");
    assert_eq!(ruling["telemetry_egress_cleared"], true, "{ruling}");
    assert_eq!(crossings[1]["event"], "crossing_refused");
    drop(service);

    relay_ok(home.path());
    let delivered = delivered_events(&hub);
    assert!(
        delivered.contains(&("content_retrieved".to_owned(), page.clone()))
            && delivered.contains(&("content_grounded".to_owned(), page.clone())),
        "{delivered:?}"
    );
    assert!(
        delivered.iter().all(|(_, url)| url == &page),
        "{delivered:?}"
    );
    // Review F3: the refusal leaves only in the session's refused count, and
    // only under the declared scope's clearance, since the consent covers the
    // demanded crossing alone.
    let batches = hub.batches.lock().expect("batches");
    assert!(!batches.is_empty());
    for batch in batches.iter() {
        assert_eq!(batch["refused"], 1, "{batch}");
    }
}

/// EDG-88: with no declared scope, a hosted session behaves as before. It
/// has no directory and runs under the top-level policy, so a host the
/// pilot scope would refuse is admitted; the relay sends nothing of that
/// crossing. `status` and `doctor` say the service declares no scope.
#[test]
fn a_hosted_service_without_a_declared_scope_runs_its_sessions_under_the_top_level_policy() {
    let hub = Hub::start();
    let home = tempfile::tempdir().expect("home");
    let scope = tempfile::tempdir().expect("scope");
    let directory = scope.path().canonicalize().expect("canonical");
    hub.enrol_managed(home.path());
    hub.publish(1, pilot_policy(&directory, json!({})));
    configure_scoped(home.path(), None);
    let site = undemanding_site();
    let service = hosted_service(home.path());
    let auth = hub.bearer("user-1", "m365-copilot");
    let session = service.open_session("m365-copilot", &auth);
    let status = payload(&service.call(
        "m365-copilot",
        &auth,
        &session,
        1,
        "context_status",
        json!({}),
    ));
    assert_eq!(status["cwd"], Value::Null, "{status}");
    assert_eq!(status["policy"]["scope"], Value::Null);
    let other = format!(
        "{}/article",
        site.url().replace("127.0.0.1", "publisher.test")
    );
    let admitted = service.call(
        "m365-copilot",
        &auth,
        &session,
        2,
        "context_fetch",
        json!({"url": other}),
    );
    assert_eq!(
        body(&admitted)["result"]["isError"],
        false,
        "{}",
        text(&admitted)
    );
    assert!(
        !session_records(home.path(), &session)
            .iter()
            .any(|record| record["event"] == "hosted_scope")
    );
    for command in ["status", "doctor"] {
        let (_, stdout, stderr) = run(home.path(), &[command]);
        assert!(
            stdout.contains(
                "no scope declared: sessions have no directory and run under the top-level policy"
            ),
            "{command}: {stdout}{stderr}"
        );
    }
    drop(service);
    relay_ok(home.path());
    assert_eq!(delivered_events(&hub), []);
}

/// EDG-88: a declared directory must be absolute and exist, or the
/// configuration does not load; one that no scope of the policy in force
/// and no enrolled directory selects opens no session, the answer and
/// `status` naming the gap, rather than running the session under the
/// top-level policy.
#[test]
fn a_declared_scope_nothing_selects_opens_no_session() {
    let hub = Hub::start();
    let home = tempfile::tempdir().expect("home");
    let scope = tempfile::tempdir().expect("scope");
    let unselected = tempfile::tempdir().expect("unselected");
    let directory = scope.path().canonicalize().expect("canonical");
    hub.enrol_managed(home.path());
    hub.publish(1, pilot_policy(&directory, json!({})));
    for (declared, fault) in [
        (Path::new("relative/dir"), "must be an absolute path"),
        (
            Path::new("/nonexistent/commonmeasure-scope"),
            "cannot resolve directory",
        ),
    ] {
        configure_scoped(home.path(), Some(declared));
        let output = service_command(home.path()).output().expect("process");
        assert!(!output.status.success(), "{declared:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("session_directory") && stderr.contains(fault),
            "{stderr}"
        );
    }

    let unselected = unselected.path().canonicalize().expect("canonical");
    configure_scoped(home.path(), Some(&unselected));
    let service = hosted_service(home.path());
    let auth = hub.bearer("user-1", "m365-copilot");
    let session = service.open_session("m365-copilot", &auth);
    let response = service.call(
        "m365-copilot",
        &auth,
        &session,
        1,
        "context_status",
        json!({}),
    );
    assert_eq!(response.status, 500, "{}", text(&response));
    assert!(
        text(&response)
            .contains("which no scope of the policy in force and no enrolled directory selects"),
        "{}",
        text(&response)
    );
    let (_, stdout, _) = run(home.path(), &["status"]);
    assert!(
        stdout.contains(&format!(
            "declares session_directory {}, which no scope",
            unselected.display()
        )),
        "{stdout}"
    );
}

/// EDG-88: a declared scope and a principal binding together, as the
/// contract states. The session resolves its policy for the bearer's
/// subject: a bound subject gets the binding's overlay and then the
/// declared scope's, which it owns; an unbound subject fails closed and is
/// refused, as it would be with no declared scope. The relay resolves
/// clearance under the service's OS principal, which no binding names, so
/// the scope's clearance does not reach it; the crossing met under the
/// reporting consent is delivered by that consent.
#[test]
fn a_declared_scope_and_a_principal_binding_resolve_as_the_contract_says() {
    let hub = Hub::start();
    let home = tempfile::tempdir().expect("home");
    let scope = tempfile::tempdir().expect("scope");
    let directory = scope.path().canonicalize().expect("canonical");
    hub.enrol_managed(home.path());
    let mut policy = pilot_policy(
        &directory,
        json!({"principals": [{"principal": "alice", "subject": "user-1"}]}),
    );
    policy["scopes"][0]["principal"] = json!("alice");
    hub.publish(1, policy);
    configure_scoped(home.path(), Some(&directory));
    agree(home.path());
    let site = commonmeasure_site();
    let service = hosted_service(home.path());
    let page = getting_started(&site);

    let alice = hub.bearer("user-1", "m365-copilot");
    let session = service.open_session("m365-copilot", &alice);
    let status = payload(&service.call(
        "m365-copilot",
        &alice,
        &session,
        1,
        "context_status",
        json!({}),
    ));
    assert_eq!(status["policy"]["principal"], "alice", "{status}");
    assert_eq!(
        status["policy"]["scope"],
        directory.to_str().expect("UTF-8")
    );
    assert_eq!(status["policy"]["fail_closed"], Value::Null);
    let fetched = service.call(
        "m365-copilot",
        &alice,
        &session,
        2,
        "context_fetch",
        json!({"url": page}),
    );
    assert_eq!(
        body(&fetched)["result"]["isError"],
        false,
        "{}",
        text(&fetched)
    );

    let stranger = hub.bearer("user-2", "m365-copilot");
    let other = service.open_session("m365-copilot", &stranger);
    let refused = service.call(
        "m365-copilot",
        &stranger,
        &other,
        1,
        "context_fetch",
        json!({"url": page}),
    );
    assert_eq!(
        body(&refused)["result"]["isError"],
        true,
        "{}",
        text(&refused)
    );
    assert!(
        text(&refused).contains("has no principal binding"),
        "{}",
        text(&refused)
    );
    drop(service);

    relay_ok(home.path());
    assert_eq!(
        delivered_events(&hub),
        [
            ("content_retrieved".to_owned(), page.clone()),
            ("content_grounded".to_owned(), page.clone()),
        ]
    );
}

/// Review F2 (P1) on a hosted session under a declared scope. The session
/// first reads the `commonmeasure.ai` page whose licence demands reporting,
/// admitted with the demand met, then a page of `institution.test`, whose
/// operator terms need `access_context` and whose own demand nothing makes;
/// the scope admits both hosts. The relay delivers the first page's
/// retrieval and grounding and holds the second crossing alone.
#[test]
fn a_hosted_session_keeps_its_earlier_report_when_a_later_crossing_is_held() {
    let hub = Hub::start();
    let home = tempfile::tempdir().expect("home");
    let scope = tempfile::tempdir().expect("scope");
    let directory = scope.path().canonicalize().expect("canonical");
    hub.enrol_managed(home.path());
    let mut policy = pilot_policy(
        &directory,
        json!({"terms": [{"host": "institution.test", "reference": "institution-7",
                          "access_context": [{"scheme": "ror", "value": "https://ror.org/013meh722"}]}]}),
    );
    policy["scopes"][0]["constraints"]
        .as_array_mut()
        .expect("constraints")
        .push(json!({"kind": "allowed_source_host", "host": "institution.test"}));
    hub.publish(1, policy);
    configure_scoped(home.path(), Some(&directory));
    agree(home.path());
    let site = commonmeasure_site();
    let plain = undemanding_site();
    let service = hosted_service(home.path());
    let auth = hub.bearer("user-1", "m365-copilot");
    let session = service.open_session("m365-copilot", &auth);
    let page = getting_started(&site);
    let held = format!(
        "{}/page",
        plain.url().replace("127.0.0.1", "institution.test")
    );
    for (id, url) in [(1, &page), (2, &held)] {
        let fetched = service.call(
            "m365-copilot",
            &auth,
            &session,
            id,
            "context_fetch",
            json!({"url": url}),
        );
        assert_eq!(
            body(&fetched)["result"]["isError"],
            false,
            "{}",
            text(&fetched)
        );
    }
    drop(service);

    let (ok, stdout, stderr) = run(home.path(), &["relay"]);
    assert!(ok, "{stdout}{stderr}");
    assert_eq!(
        delivered_events(&hub),
        [
            ("content_retrieved".to_owned(), page.clone()),
            ("content_grounded".to_owned(), page.clone()),
        ],
        "{stdout}"
    );
    assert!(
        stdout
            .contains("1 crossing withheld: operator terms for their hosts require access_context"),
        "{stdout}"
    );
}
