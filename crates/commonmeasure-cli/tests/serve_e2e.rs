//! The rebuilt operator console driven as a person uses it: the real binary
//! listening on a real loopback socket, evidence recorded by the real hook
//! path, and HTTP requests over `commonmeasure_http`'s client. The console renders the
//! maud app shell; these tests exercise it end to end, not a render function.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};

use commonmeasure_http::{Request, send};
use serde_json::{Value, json};

struct Console {
    child: Child,
    base: String,
}

impl Console {
    /// Start `commonmeasure serve` on an OS-chosen port and wait for it to name
    /// the address it landed on.
    fn start(home: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
            .args(["serve", "--listen", "127.0.0.1:0"])
            .env("COMMONMEASURE_HOME", home)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the binary should start");
        let stdout = child.stdout.as_mut().expect("stdout");
        let mut lines = BufReader::new(stdout).lines();
        let base = loop {
            let line = lines
                .next()
                .expect("serve announces its address before serving")
                .expect("readable stdout");
            if let Some(address) = line.strip_prefix("listening on ") {
                break address.trim().to_owned();
            }
        };
        Self { child, base }
    }

    fn get(&self, path: &str) -> commonmeasure_http::Response {
        send(&format!("{}{path}", self.base), Request::get("/")).expect("request should complete")
    }

    fn text(&self, path: &str) -> String {
        let response = self.get(path);
        assert_eq!(response.status, 200, "{path} should answer 200");
        String::from_utf8(response.body).expect("responses are UTF-8")
    }

    fn get_json(&self, path: &str) -> Value {
        let response = self.get(path);
        assert_eq!(response.status, 200, "{path} should answer 200");
        serde_json::from_slice(&response.body).expect("the API answers JSON")
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Record one witnessed crossing through the real hook path.
fn record_crossing(home: &Path, session: &str, url: &str, cwd: Option<&str>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["hook", "post-tool-use"])
        .env("COMMONMEASURE_HOME", home)
        .stdin(Stdio::piped())
        .spawn()
        .expect("hook should start");
    writeln!(
        child.stdin.as_mut().unwrap(),
        "{}",
        json!({
            "session_id": session,
            "hook_event_name": "PostToolUse",
            "cwd": cwd,
            "tool_name": "WebFetch",
            "tool_input": {"url": url},
            "tool_response": {"result": "The cap is set quarterly."}
        })
    )
    .unwrap();
    assert!(child.wait().expect("wait").success());
}

#[test]
fn the_console_serves_its_five_sections_and_the_record_from_one_binary() {
    let home = tempfile::tempdir().expect("home");
    record_crossing(
        home.path(),
        "s-console",
        "https://www.ofgem.gov.uk/cap",
        Some("/home/op/code/ozone"),
    );
    let console = Console::start(home.path());

    // `/` lands on the Overview; every section is reachable through the shell.
    let overview = console.text("/");
    assert!(overview.contains("class=\"app\""));
    assert!(overview.contains("Overview"));
    assert!(overview.contains("crossings recorded"));

    for (path, marker) in [
        ("/app/record", "Record"),
        ("/app/policy", "Policy"),
        ("/app/sources", "Sources"),
        ("/app/compare", "Compare"),
    ] {
        assert!(console.text(path).contains(marker), "{path} lost {marker}");
    }

    // The Record rail lists the recorded session and links its detail; the
    // detail fragment loads that session's crossing.
    let record = console.text("/app/record");
    assert!(record.contains("s-console"));
    assert!(record.contains("/app/fragments/session/s-console"));
    let detail = console.text("/app/fragments/session/s-console");
    assert!(detail.contains("https://www.ofgem.gov.uk/cap"));

    // Sources reports providers with no key on a home that has no credentials.
    assert!(console.text("/app/sources").contains("no key"));
    // No policy declared here, and the screen says so rather than faking a table.
    assert!(
        console
            .text("/app/policy")
            .contains("No policy is declared")
    );
}

#[test]
fn the_json_api_serves_the_same_evidence_the_pages_render() {
    let home = tempfile::tempdir().expect("home");
    record_crossing(
        home.path(),
        "s-api",
        "https://www.gov.uk/energy-price-cap",
        Some("/home/op/code/ozone"),
    );
    let console = Console::start(home.path());

    let sessions = console.get_json("/api/sessions");
    let session = &sessions.as_array().expect("a session list")[0];
    assert_eq!(session["session_id"], "s-api");
    assert_eq!(session["observed"], 1);
    assert_eq!(session["reconstructed"], 0);
    assert_eq!(session["grounded_witnessed"], 1);

    let records = console.get_json("/api/sessions/s-api");
    assert_eq!(records.as_array().expect("records").len(), 1);

    assert_eq!(console.get("/api/status").status, 200);
}

#[test]
fn evidence_recorded_while_serving_is_visible_on_the_next_request() {
    let home = tempfile::tempdir().expect("home");
    let console = Console::start(home.path());

    // Nothing yet.
    assert!(
        console
            .get_json("/api/sessions")
            .as_array()
            .expect("a list")
            .is_empty()
    );

    // Record after the server started; the reader's request is the poll.
    record_crossing(home.path(), "s-live", "https://a.example/x", None);
    let sessions = console.get_json("/api/sessions");
    assert_eq!(sessions.as_array().expect("a list").len(), 1);
    assert_eq!(sessions[0]["session_id"], "s-live");
}

/// Post a form to the console as its own page would, with or without the
/// htmx request header.
fn post_form(
    console: &Console,
    path: &str,
    body: &str,
    htmx: bool,
) -> commonmeasure_http::Response {
    let mut request = Request::post(
        "/",
        body.as_bytes().to_vec(),
        "application/x-www-form-urlencoded",
    );
    if htmx {
        request.headers.set("HX-Request", "true");
    }
    send(&format!("{}{path}", console.base), request).expect("request should complete")
}

fn body_text(response: commonmeasure_http::Response) -> String {
    String::from_utf8(response.body).expect("responses are UTF-8")
}

fn policy_revision(console: &Console) -> String {
    console.get_json("/api/policy")["revision"]
        .as_str()
        .expect("a declared policy has a revision")
        .to_owned()
}

fn write_policy(home: &Path, policy: &str) {
    std::fs::write(home.join("policy.json"), policy).expect("policy");
}

const CLIENT_POLICY: &str = r#"{
  "policy_mode": "observe",
  "constraints": [{"kind": "denied_source_host", "host": "tracker.example"}],
  "scopes": [
    {"match": "code/client", "engagement": "client-a", "policy_mode": "strict"},
    {"match": "code/personal", "engagement": "personal"}
  ]
}"#;

/// The Policy screen names each scope where the engagement the policy
/// declares and the engagement the attribution rules report disagree, with
/// both names, and says when none do.
#[test]
fn the_policy_screen_names_each_scope_whose_two_engagement_names_differ() {
    let home = tempfile::tempdir().expect("home");
    write_policy(home.path(), CLIENT_POLICY);
    std::fs::write(
        home.path().join("attribution.json"),
        r#"{"rules":[{"match":"code/client","engagement":"client-b"},{"match":"code/personal","engagement":"personal"}]}"#,
    )
    .unwrap();
    record_crossing(
        home.path(),
        "s-client",
        "https://www.gov.uk/x",
        Some("/home/op/code/client/api"),
    );
    record_crossing(
        home.path(),
        "s-personal",
        "https://www.gov.uk/y",
        Some("/home/op/code/personal"),
    );
    let console = Console::start(home.path());

    let api = console.get_json("/api/policy");
    let divergences = api["engagement_divergences"].as_array().expect("a list");
    assert_eq!(divergences.len(), 1, "{api}");
    assert_eq!(divergences[0]["scope"], "code/client");
    assert_eq!(divergences[0]["governing"], "client-a");
    assert_eq!(divergences[0]["reported"], "client-b");

    let page = console.text("/app/policy");
    assert!(
        page.contains(r#"id="divergences""#),
        "the screen lists the disagreement"
    );
    assert!(page.contains("client-a") && page.contains("client-b"));

    // Repair the rule through the console; the disagreement is gone on the
    // next read and no evidence changed.
    let rules_revision =
        commonmeasure_harness::declaration::revision(&home.path().join("attribution.json"))
            .unwrap();
    let response = post_form(
        &console,
        "/app/attribution",
        &format!(
            "revision={rules_revision}&match=code%2Fclient&engagement=client-a&match=code%2Fpersonal&engagement=personal&match=&engagement="
        ),
        false,
    );
    assert_eq!(response.status, 200);
    let page = body_text(response);
    assert!(page.contains(r#"data-outcome="saved""#), "{page}");
    assert!(page.contains("Every scope's governing engagement"));
    assert!(
        console.get_json("/api/policy")["engagement_divergences"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        console.get_json("/api/sessions/s-client")[0]["payload"]["cwd"],
        "/home/op/code/client/api",
        "the evidence is untouched"
    );
}

/// The mode dial writes through the runtime's loader, and the saved file is
/// the one the runtime then resolves. The answer states what the edit reaches.
#[test]
fn a_mode_write_saves_through_the_loader_and_states_its_reach() {
    let home = tempfile::tempdir().expect("home");
    write_policy(home.path(), CLIENT_POLICY);
    let console = Console::start(home.path());
    let revision = policy_revision(&console);

    let response = post_form(
        &console,
        "/app/policy/mode",
        &format!("scope=code%2Fpersonal&mode=strict&revision={revision}"),
        true,
    );
    assert_eq!(response.status, 200);
    let fragment = body_text(response);
    assert!(fragment.contains(r#"data-outcome="saved""#), "{fragment}");
    assert!(fragment.contains("now runs in strict mode"));
    assert!(fragment.contains("governs the next crossing and nothing already recorded"));
    assert!(
        fragment.starts_with("<section"),
        "an htmx post gets the screen alone"
    );
    assert!(
        console
            .text("/app/policy")
            .contains(r#"name="htmx-config""#),
        "the page tells htmx to swap every status in"
    );

    let reread = commonmeasure_harness::policy::PolicyDocument::read(home.path()).unwrap();
    assert_eq!(
        reread.resolve(Some("/home/op/code/personal")).mode(),
        commonmeasure_harness::policy::PolicyMode::Strict
    );
    assert_eq!(
        reread
            .resolve(Some("/home/op/code/client"))
            .governing_engagement(),
        Some("client-a"),
        "the rest of the declaration was carried through"
    );
    assert_ne!(policy_revision(&console), revision);

    // A word the vocabulary lacks is refused before the file is touched.
    let revision = policy_revision(&console);
    let response = post_form(
        &console,
        "/app/policy/mode",
        &format!("scope=&mode=lenient&revision={revision}"),
        false,
    );
    assert_eq!(response.status, 400);
    assert!(body_text(response).contains("not a policy mode"));
    assert_eq!(policy_revision(&console), revision);

    // The same refusal over htmx carries the same status and the screen.
    let response = post_form(
        &console,
        "/app/policy/mode",
        &format!("scope=&mode=lenient&revision={revision}"),
        true,
    );
    assert_eq!(response.status, 400);
    let fragment = body_text(response);
    assert!(fragment.starts_with("<section"));
    assert!(fragment.contains(r#"data-outcome="refused""#));
}

/// A write against a revision that is no longer on disk is refused with both
/// revisions stated, and nothing is written.
#[test]
fn a_conflicting_edit_is_refused_with_both_versions_stated() {
    let home = tempfile::tempdir().expect("home");
    write_policy(home.path(), CLIENT_POLICY);
    let console = Console::start(home.path());
    let stale = policy_revision(&console);

    // Another editor changes the file underneath.
    let mut edited = CLIENT_POLICY.to_owned();
    edited = edited.replace(r#""policy_mode": "observe""#, r#""policy_mode": "prefer""#);
    write_policy(home.path(), &edited);
    let current = policy_revision(&console);
    assert_ne!(stale, current);

    let response = post_form(
        &console,
        "/app/policy/mode",
        &format!("scope=&mode=strict&revision={stale}"),
        false,
    );
    assert_eq!(response.status, 409);
    let page = body_text(response);
    assert!(page.contains(r#"data-outcome="conflict""#), "{page}");
    assert!(page.contains(&stale[..12]) && page.contains(&current[..12]));
    assert!(
        page.contains(r#"value="prefer" checked"#),
        "the page shows the current declaration"
    );
    assert_eq!(policy_revision(&console), current, "nothing was written");
    assert_eq!(
        commonmeasure_harness::policy::PolicyDocument::read(home.path())
            .unwrap()
            .resolve(None)
            .mode(),
        commonmeasure_harness::policy::PolicyMode::Prefer
    );
}

/// With no policy file the console refuses to write one, and says so.
#[test]
fn a_write_never_creates_a_policy_where_none_is_declared() {
    let home = tempfile::tempdir().expect("home");
    let console = Console::start(home.path());
    let response = post_form(
        &console,
        "/app/policy/deny",
        "scope=&host=tracker.example&revision=absent",
        false,
    );
    assert_eq!(response.status, 409);
    assert!(body_text(response).contains("never creates one"));
    assert!(!home.path().join("policy.json").exists());
}

/// A denied host is forecast over the record before it is saved, with each
/// grade on its own line, and the save into an inheriting scope says the
/// scope has stopped inheriting.
#[test]
fn a_draft_denied_host_is_forecast_by_grade_and_then_saved_into_its_scope() {
    let home = tempfile::tempdir().expect("home");
    write_policy(home.path(), CLIENT_POLICY);
    // Two witnessed crossings through the real hook path, in the scope the
    // draft is for and outside it.
    record_crossing(
        home.path(),
        "s-forecast",
        "https://cdn.beacon.example/pixel",
        Some("/home/op/code/client/api"),
    );
    record_crossing(
        home.path(),
        "s-forecast",
        "https://www.gov.uk/x",
        Some("/home/op/code/client/api"),
    );
    record_crossing(
        home.path(),
        "s-elsewhere",
        "https://cdn.beacon.example/pixel",
        Some("/home/op/code/personal"),
    );
    // One reconstructed crossing, imported from a real transcript.
    let claude_home = tempfile::tempdir().expect("claude home");
    let project = claude_home
        .path()
        .join(".claude/projects/-home-op-code-client");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        project.join("22222222-2222-4222-8222-222222222222.jsonl"),
        concat!(
            r#"{"type":"assistant","timestamp":"2026-08-01T10:00:00Z","cwd":"/home/op/code/client","message":{"content":[{"type":"tool_use","id":"t1","name":"WebFetch","input":{"url":"https://cdn.beacon.example/old"}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-08-01T10:00:01Z","cwd":"/home/op/code/client","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"pixel"}]}}"#,
            "\n"
        ),
    )
    .unwrap();
    let imported = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .arg("import")
        .env("HOME", claude_home.path())
        .env("COMMONMEASURE_HOME", home.path())
        .output()
        .expect("import runs");
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );

    let console = Console::start(home.path());
    let sessions = console.get_json("/api/sessions");
    let reconstructed: u64 = sessions
        .as_array()
        .unwrap()
        .iter()
        .map(|session| session["reconstructed"].as_u64().unwrap_or(0))
        .sum();
    assert_eq!(reconstructed, 1, "{sessions}");

    let forecast = console
        .text("/app/policy/forecast?scope=code%2Fclient&host=cdn.beacon.example&action=deny");
    assert!(
        forecast.contains("Nothing is saved by a forecast"),
        "{forecast}"
    );
    // Witnessed: two in the client scope, one of which the draft refuses
    // (strict); the personal-scope crossing is not governed by the draft.
    let row = |grade: &str| -> Vec<u64> {
        let start = forecast.find(&format!("<td>{grade}</td>")).expect(grade);
        forecast[start..]
            .split("<td class=\"mono\">")
            .skip(1)
            .take(5)
            .map(|cell| cell.split('<').next().unwrap().trim().parse().unwrap())
            .collect()
    };
    let witnessed = row("witnessed");
    assert_eq!(witnessed[0], 3, "witnessed crossings: {forecast}");
    assert_eq!(witnessed[1], 1, "newly refused: {forecast}");
    assert_eq!(witnessed[4], 2, "unchanged: {forecast}");
    let reconstructed = row("reconstructed");
    assert_eq!(reconstructed[0], 1);
    assert_eq!(
        reconstructed[1], 0,
        "the importer records no working directory on a reconstructed crossing, whatever the \
         transcript line carries, so it is judged under the top-level policy and not the \
         client scope; it is counted on its own line and never with the witnessed ones"
    );
    assert!(forecast.contains("Reconstructed crossings carry no working directory"));
    assert!(
        forecast.contains("Judged as principal os-user:"),
        "{forecast}"
    );
    assert!(forecast.contains("0 of these crossings were recorded under another principal"));
    assert!(forecast.contains("0 crossing records carry no URL"));
    assert!(forecast.contains("https://cdn.beacon.example/pixel"));
    assert!(forecast.contains("denies host cdn.beacon.example"));
    assert_eq!(
        policy_revision(&console).len(),
        64,
        "the forecast wrote nothing"
    );

    // A mode draft is forecast the same way. Before the host is denied the
    // strict client scope refuses nothing, so relaxing it changes nothing.
    let relaxed = console.text("/app/policy/forecast?scope=code%2Fclient&mode=observe&action=mode");
    assert!(relaxed.contains("running in observe mode"), "{relaxed}");
    assert!(relaxed.contains("<td>witnessed</td>"));

    let revision = policy_revision(&console);
    let response = post_form(
        &console,
        "/app/policy/deny",
        &format!("scope=code%2Fclient&host=cdn.beacon.example&revision={revision}"),
        false,
    );
    assert_eq!(response.status, 200);
    let page = body_text(response);
    assert!(page.contains(r#"data-outcome="saved""#), "{page}");
    assert!(page.contains("now denies host cdn.beacon.example"));
    assert!(
        page.contains("was inheriting the top-level constraints"),
        "the page says the scope has stopped inheriting: {page}"
    );
    let reread = commonmeasure_harness::policy::PolicyDocument::read(home.path()).unwrap();
    let governed = reread.resolve(Some("/home/op/code/client/api"));
    assert!(
        governed
            .admit_host("https://cdn.beacon.example/pixel")
            .is_refusal()
    );
    assert!(
        governed
            .admit_host("https://tracker.example/x")
            .is_refusal(),
        "the inherited denial was kept as the scope's own"
    );
    assert!(
        !reread
            .resolve(Some("/home/op/code/personal"))
            .admit_host("https://cdn.beacon.example/pixel")
            .is_refusal(),
        "the other scope is untouched"
    );

    // Now that the host is denied, relaxing the scope to observe would turn
    // that refusal into a carried breach, and the forecast says so.
    let relaxed = console.text("/app/policy/forecast?scope=code%2Fclient&mode=observe&action=mode");
    let breached: Vec<u64> = {
        let start = relaxed.find("<td>witnessed</td>").unwrap();
        relaxed[start..]
            .split("<td class=\"mono\">")
            .skip(1)
            .take(5)
            .map(|cell| cell.split('<').next().unwrap().trim().parse().unwrap())
            .collect()
    };
    assert_eq!(breached[2], 1, "newly a breach: {relaxed}");

    // A draft access rule is forecast the same way, with the runtime's own
    // reason on the example and the JSON to add; a pattern the vocabulary
    // refuses is refused here too.
    let rule = console.text("/app/policy/forecast?scope=code%2Fclient&host=*.gov.uk&action=refuse");
    assert!(rule.contains("*.gov.uk → refuse"), "{rule}");
    assert!(rule.contains("https://www.gov.uk/x"), "{rule}");
    assert!(
        rule.contains("(*.gov.uk) refuses host www.gov.uk"),
        "{rule}"
    );
    assert!(
        rule.contains(r#"&quot;kind&quot;:&quot;access_rule&quot;"#),
        "{rule}"
    );
    assert!(
        rule.contains(r#"&quot;host&quot;:&quot;*.gov.uk&quot;"#),
        "{rule}"
    );
    let bad = console.text("/app/policy/forecast?scope=code%2Fclient&host=a.*.b&action=refuse");
    assert!(bad.contains("No forecast: "), "{bad}");
}

/// A write carrying a foreign origin is refused before any form is read: the
/// same guard the Compare query stands under.
#[test]
fn a_cross_origin_policy_write_is_refused() {
    let home = tempfile::tempdir().expect("home");
    write_policy(home.path(), CLIENT_POLICY);
    let console = Console::start(home.path());
    let revision = policy_revision(&console);
    let mut request = Request::post(
        "/",
        format!("scope=&mode=strict&revision={revision}").into_bytes(),
        "application/x-www-form-urlencoded",
    );
    request.headers.set("Origin", "https://evil.example");
    let response = send(&format!("{}/app/policy/mode", console.base), request).unwrap();
    assert_eq!(response.status, 403);
    assert_eq!(policy_revision(&console), revision);
}

/// A form this console did not render, with a match and no engagement after
/// it, is refused by name and nothing is written.
#[test]
fn an_unpaired_attribution_field_is_refused_by_name() {
    let home = tempfile::tempdir().expect("home");
    let console = Console::start(home.path());
    let response = post_form(
        &console,
        "/app/attribution",
        "revision=absent&match=code%2Fozone&engagement=ozone&match=tail",
        false,
    );
    assert_eq!(response.status, 400);
    let page = body_text(response);
    assert!(page.contains(r#"data-outcome="refused""#), "{page}");
    assert!(
        page.contains("has no &quot;engagement&quot; after it"),
        "{page}"
    );
    assert!(!home.path().join("attribution.json").exists());
}

/// One mediated crossing through the real MCP server against a loopback
/// origin, recorded into `home`, under a policy that admits private hosts.
fn record_mediated_crossing(home: &Path, session: &str, url: &str) {
    write_policy(
        home,
        r#"{"policy_mode": "observe", "allow_private_hosts": true}"#,
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["mcp", "--host", "claude-code", "--session", session])
        .env("COMMONMEASURE_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the binary should start");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        for request in [
            json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {}}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                   "params": {"name": "context_fetch", "arguments": {"url": url}}}),
        ] {
            writeln!(stdin, "{request}").expect("write request");
        }
    }
    assert!(child.wait().expect("wait").success());
}

/// The Record pane shows what the record carries and nothing else: a
/// mediated fetch renders both hashes, the HTTP status and the identity the
/// request presented; an observed crossing renders its content hash alone,
/// with no status and no identity. Read back from the log the real binary
/// wrote, so the strings asserted are the record's own.
#[test]
fn the_record_pane_shows_the_hashes_status_and_identity_the_record_carries() {
    let origin = commonmeasure_http::Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(|_| commonmeasure_http::Response::text(200, "The cap is set quarterly."))
        .expect("spawn");
    let home = tempfile::tempdir().expect("home");
    record_mediated_crossing(home.path(), "local-mediated", &origin.url());
    record_crossing(home.path(), "s-observed", "https://www.gov.uk/x", None);

    let log = std::fs::read_to_string(home.path().join("sessions/local-mediated.ndjson")).unwrap();
    let crossing: Value = log
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("NDJSON"))
        .find(|record| record["event"] == "crossing_mediated")
        .expect("the fetch was recorded");
    let payload = &crossing["payload"];
    let retrieved = payload["retrieved_hash"].as_str().expect("retrieved_hash");
    let content = payload["content_hash"].as_str().expect("content_hash");
    assert_eq!(payload["http_status"], 200);
    let reason = payload["identity"]["unsigned"]
        .as_str()
        .expect("an edge that is not enrolled records why it is unsigned");

    let console = Console::start(home.path());
    let pane = console.text("/app/fragments/session/local-mediated");
    assert!(pane.contains(retrieved), "{pane}");
    assert!(pane.contains(content), "{pane}");
    assert!(
        pane.contains("<dt>bytes received</dt>") && pane.contains("<dt>text read</dt>"),
        "{pane}"
    );
    assert!(
        pane.contains("<dt>status</dt><dd class=\"mono\">200</dd>"),
        "{pane}"
    );
    assert!(pane.contains("<dt>identity</dt><dd>unsigned — "), "{pane}");
    let reason_head = reason.split(':').next().unwrap();
    assert!(pane.contains(reason_head), "{pane}");

    let observed_log =
        std::fs::read_to_string(home.path().join("sessions/s-observed.ndjson")).unwrap();
    let observed: Value = serde_json::from_str(observed_log.lines().next().unwrap()).unwrap();
    let observed_hash = observed["payload"]["content_hash"]
        .as_str()
        .expect("content_hash");
    let pane = console.text("/app/fragments/session/s-observed");
    assert!(pane.contains(observed_hash), "{pane}");
    assert!(pane.contains("<dt>text read</dt>"), "{pane}");
    assert!(!pane.contains("<dt>bytes received</dt>"), "{pane}");
    assert!(!pane.contains("<dt>status</dt>"), "{pane}");
    assert!(!pane.contains("<dt>identity</dt>"), "{pane}");

    // The same session as a page, for a link followed with no script: the
    // rail marks it and the pane holds the same crossing.
    let page = console.text("/app/record?session=local-mediated");
    assert!(page.contains(r#"aria-current="true""#), "{page}");
    assert!(page.contains(retrieved), "{page}");
    assert!(page.contains("<h1>Record</h1>"), "{page}");
    assert!(
        page.contains(home.path().join("sessions").to_str().unwrap()),
        "the page names the record it read"
    );

    // An unknown console route answers a page, not JSON; the API still answers JSON.
    let missing = console.get("/app/nowhere");
    assert_eq!(missing.status, 404);
    let body = String::from_utf8(missing.body).unwrap();
    assert!(body.contains("<h1>404 Not found</h1>"), "{body}");
    assert!(body.contains(r#"lang="en-GB""#));
    let api = console.get("/api/nowhere");
    assert_eq!(api.status, 404);
    assert!(String::from_utf8(api.body).unwrap().starts_with('{'));
}
