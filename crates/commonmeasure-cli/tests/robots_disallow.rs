//! A `robots.txt` `Disallow` binds in every policy mode (owner decision, 14
//! September 2026), driven through the real binary over loopback origins. The modes vary and the
//! outcome does not: a disallowed page is refused before it is requested,
//! and the origin's own log shows that nothing further was asked of it.
//!
//! Each origin's `robots.txt` also names a licence and states a delay, so a
//! refusal that still read the licence, or asked for the Content Telemetry
//! manifest at the crossing's free turn, shows up in the origin's log.
//!
//! A `robots.txt` that cannot be read follows RFC 9309 §2.3.1 in every mode
//! (owner decision, 22 September 2026): a 4xx other than 429 states no
//! rules; a 429, a 5xx or a timeout is unreachable, and the crossing is ruled
//! by the last answer the host gave, however old, or refused as a complete
//! disallow where none is held.
//!
//! RFC 9309 §2.5 and §2.3.1.2 in every mode (owner decision, 22 September
//! 2026): a `robots.txt` over the 512 KiB parsing limit is read from its
//! complete lines within it, and a `robots.txt` redirect this edge declines
//! to follow, or a chain past five redirects, leaves the file unreachable.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use commonmeasure_http::{Response, Server, ServerHandle};
use serde_json::{Value, json};

const MODES: [&str; 3] = ["strict", "prefer", "observe"];

/// A licence that permits AI input and demands nothing.
const LICENCE: &str = r#"<rsl xmlns="https://rslstandard.org/rsl">
  <content url="/"><license><permits type="usage">ai-input</permits></license></content></rsl>"#;

/// The group naming the product token disallows `/members/`; the wildcard
/// group allows everything.
const NAMED_DISALLOW: &str = "\
License: /license.xml

User-agent: *
Allow: /
Crawl-delay: 1

User-agent: CommonMeasureBot
Allow: /
Disallow: /members/
Crawl-delay: 1
";

/// No group names the product token, and the wildcard group disallows
/// everything.
const WILDCARD_DISALLOW: &str = "\
License: /license.xml

User-agent: *
Disallow: /
Crawl-delay: 1
";

/// The wildcard group disallows everything; the group naming the product
/// token allows it, and stands over the wildcard group.
const NAMED_ALLOW_OVER_WILDCARD: &str = "\
User-agent: *
Disallow: /

User-agent: CommonMeasureBot
Allow: /
";

const REFUSED: &str = "A `Disallow` binds in every policy mode: refused before the request.";

struct Origin {
    handle: ServerHandle,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Origin {
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.handle.addr().port())
    }

    /// Every request this origin answered, in order: what a publisher's own
    /// log holds, probes included.
    fn requests(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

fn origin(robots: &'static str) -> Origin {
    answering(move || Response::text(200, robots))
}

/// An origin whose `robots.txt` answers as `robots` says.
fn answering(robots: impl Fn() -> Response + Send + Sync + 'static) -> Origin {
    serving(move |target| match target {
        "/robots.txt" => robots(),
        "/license.xml" => Response::new(200, LICENCE.as_bytes().to_vec()),
        _ => Response::text(200, "the page text"),
    })
}

/// An origin that answers each request target as `answer` says, and logs it.
fn serving(answer: impl Fn(&str) -> Response + Send + Sync + 'static) -> Origin {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |request| {
            log.lock().unwrap().push(request.target.clone());
            answer(&request.target)
        })
        .expect("spawn");
    Origin { handle, seen }
}

fn redirect(status: u16, location: &str) -> Response {
    let mut response = Response::new(status, Vec::new());
    response.headers.set("Location", location);
    response
}

fn home_with_mode(mode: &str) -> tempfile::TempDir {
    home_with_policy(json!({"policy_mode": mode, "allow_private_hosts": true}))
}

fn home_with_policy(policy: Value) -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("tempdir");
    std::fs::write(home.path().join("policy.json"), policy.to_string()).expect("policy");
    home
}

/// One server process, asked to fetch `url` and closed.
fn fetch(home: &Path, url: &str) -> Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["mcp", "--host", "claude-code", "--session", "test-session"])
        .env("COMMONMEASURE_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the binary should start");
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "context_fetch", "arguments": {"url": url}},
    });
    writeln!(child.stdin.as_mut().expect("stdin"), "{request}").expect("write request");
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success(), "the server should exit cleanly");
    let mut responses = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("one JSON object per line"))
        .collect::<Vec<_>>();
    assert_eq!(responses.len(), 1, "{responses:?}");
    responses.remove(0)
}

fn text_of(response: &Value) -> String {
    response["result"]["content"][0]["text"]
        .as_str()
        .expect("a tool result carries text")
        .to_owned()
}

fn crossings(home: &Path) -> Vec<Value> {
    let path = home.join("sessions/test-session.ndjson");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("the log is NDJSON"))
        .filter(|record| {
            record["event"]
                .as_str()
                .is_some_and(|event| event.starts_with("crossing_"))
        })
        .collect()
}

/// The refusal a `Disallow` earns, the same in every mode: nothing but
/// `robots.txt` asked of the origin, the refusal naming the file, the group
/// and the rule, and the record carrying the `robots.txt` URL, the rule, the
/// session's mode and `refused`.
fn assert_refused(
    mode: &str,
    site: &Origin,
    page: &str,
    group: &str,
    rule: &str,
    response: &Value,
    home: &Path,
) {
    let robots_url = site.url("/robots.txt");
    assert_eq!(response["result"]["isError"], true, "{mode}: {response}");
    let detail = text_of(response);
    for needed in [
        "refused before the crossing",
        page,
        robots_url.as_str(),
        &format!("`User-agent: {group}` group"),
        &format!("`{rule}`"),
        REFUSED,
    ] {
        assert!(
            detail.contains(needed),
            "{mode}: missing {needed:?} in {detail}"
        );
    }
    assert_eq!(
        site.requests(),
        ["/robots.txt"],
        "{mode}: nothing but robots.txt was asked of the origin"
    );

    let recorded = crossings(home);
    assert_eq!(recorded.len(), 1, "{mode}");
    assert_eq!(recorded[0]["event"], "crossing_refused", "{mode}");
    let payload = &recorded[0]["payload"];
    assert_eq!(payload["url"], page, "{mode}");
    assert!(
        payload["http_status"].is_null(),
        "{mode}: nothing was requested"
    );
    assert!(payload["breach"].is_null(), "{mode}: refused, not carried");
    let refusal = payload["refusal"]
        .as_str()
        .expect("a refusal names its reason");
    assert!(
        refusal.contains(&robots_url) && refusal.contains(rule) && refusal.contains(REFUSED),
        "{mode}: {refusal}"
    );
    let robots = &payload["declarations"]["robots"];
    assert_eq!(robots["url"], robots_url, "{mode}");
    assert_eq!(robots["requested_url"], page, "{mode}");
    assert_eq!(robots["reading"]["group"], group, "{mode}");
    assert_eq!(robots["reading"]["access_rule"], rule, "{mode}");
    assert_eq!(robots["reading"]["crawlable"], false, "{mode}");
    assert_eq!(robots["mode"], mode, "{mode}");
    assert_eq!(robots["outcome"], "refused", "{mode}");
    assert!(robots["delay"].is_null(), "{mode}: no turn was taken");
    let licences = payload["declarations"]["licences"]
        .as_array()
        .expect("the licence robots.txt names is recorded");
    assert_eq!(licences.len(), 1, "{mode}");
    assert_eq!(licences[0]["cache"], "not_asked", "{mode}");
}

#[test]
fn a_disallow_in_the_group_naming_the_token_refuses_in_every_mode() {
    for mode in MODES {
        let site = origin(NAMED_DISALLOW);
        let home = home_with_mode(mode);
        let page = site.url("/members/only");
        let response = fetch(home.path(), &page);
        assert_refused(
            mode,
            &site,
            &page,
            "CommonMeasureBot",
            "Disallow: /members/",
            &response,
            home.path(),
        );
    }
}

#[test]
fn a_wildcard_disallow_refuses_in_every_mode_where_no_group_names_the_token() {
    for mode in MODES {
        let site = origin(WILDCARD_DISALLOW);
        let home = home_with_mode(mode);
        let page = site.url("/article");
        let response = fetch(home.path(), &page);
        assert_refused(
            mode,
            &site,
            &page,
            "*",
            "Disallow: /",
            &response,
            home.path(),
        );
        assert!(
            text_of(&response).contains("addresses every fetcher because no group names"),
            "{mode}: the wildcard group is named as one"
        );
    }
}

#[test]
fn a_named_group_that_allows_lets_the_fetch_go_in_every_mode_over_a_wildcard_disallow() {
    for mode in MODES {
        let site = origin(NAMED_ALLOW_OVER_WILDCARD);
        let home = home_with_mode(mode);
        let page = site.url("/article");
        let response = fetch(home.path(), &page);
        assert_eq!(response["result"]["isError"], false, "{mode}: {response}");
        assert!(
            site.requests().iter().any(|target| target == "/article"),
            "{mode}: the page was requested: {:?}",
            site.requests()
        );
        let recorded = crossings(home.path());
        assert_eq!(recorded.len(), 1, "{mode}");
        assert_eq!(recorded[0]["event"], "crossing_mediated", "{mode}");
        let robots = &recorded[0]["payload"]["declarations"]["robots"];
        assert_eq!(robots["reading"]["group"], "CommonMeasureBot", "{mode}");
        assert_eq!(robots["reading"]["access_rule"], "Allow: /", "{mode}");
        assert_eq!(robots["outcome"], "allowed", "{mode}");
        assert!(recorded[0]["payload"]["breach"].is_null(), "{mode}");
    }
}

/// Leave the answer the host gave `days_ago` in the declaration cache, long
/// past its 24-hour life: `User-agent: *` with `Disallow: /private/`.
fn hold_stale_copy(home: &Path, site: &Origin, days_ago: i64) {
    let fetched = chrono::Utc::now() - chrono::Duration::days(days_ago);
    let record = json!({
        "robots": {
            "url": site.url("/robots.txt"),
            "fetched_at": fetched,
            "expires_at": fetched + chrono::Duration::hours(24),
            "status": 200,
            "body": "User-agent: *\nDisallow: /private/\n",
        }
    });
    std::fs::create_dir_all(home.join("declarations")).expect("cache");
    std::fs::write(
        home.join("declarations/127.0.0.1.json"),
        serde_json::to_vec(&record).expect("a record"),
    )
    .expect("the cached declarations");
}

/// The refusal an unreachable file earns with nothing held: the host, the
/// failure and the rule named, no page request, and the record saying the
/// file was unreachable.
fn assert_unreachable(
    mode: &str,
    site: &Origin,
    page: &str,
    failure: &str,
    response: &Value,
    home: &Path,
) {
    assert_eq!(response["result"]["isError"], true, "{mode}: {response}");
    let detail = text_of(response);
    for needed in [
        site.url("/robots.txt").as_str(),
        page,
        "127.0.0.1",
        failure,
        "RFC 9309 section 2.3.1.4",
        "An unreachable robots.txt is a complete disallow in every policy mode",
    ] {
        assert!(
            detail.contains(needed),
            "{mode}: missing {needed:?} in {detail}"
        );
    }
    assert!(
        !site.requests().iter().any(|target| target != "/robots.txt"),
        "{mode}: only robots.txt was asked for: {:?}",
        site.requests()
    );
    let recorded = crossings(home);
    assert_eq!(recorded.len(), 1, "{mode}");
    assert_eq!(recorded[0]["event"], "crossing_refused", "{mode}");
    assert_eq!(recorded[0]["payload"]["url"], page, "{mode}");
    let robots = &recorded[0]["payload"]["declarations"]["robots"];
    assert_eq!(robots["unreachable"], true, "{mode}: {robots}");
    assert!(robots["held_copy"].is_null(), "{mode}: {robots}");
    assert!(robots["reading"].is_null(), "{mode}: {robots}");
    assert_eq!(robots["outcome"], "unreachable", "{mode}: {robots}");
    assert_eq!(robots["mode"], mode, "{mode}");
    assert!(
        robots["unavailable"]
            .as_str()
            .is_some_and(|reason| reason.contains(failure)),
        "{mode}: {robots}"
    );
}

#[test]
fn a_403_states_no_rules_and_the_fetch_goes_in_every_mode() {
    for mode in MODES {
        let site = answering(|| Response::text(403, "forbidden"));
        let home = home_with_mode(mode);
        let page = site.url("/article");
        let response = fetch(home.path(), &page);
        assert_eq!(response["result"]["isError"], false, "{mode}: {response}");
        assert!(
            site.requests().iter().any(|target| target == "/article"),
            "{mode}: {:?}",
            site.requests()
        );
        let recorded = crossings(home.path());
        assert_eq!(recorded[0]["event"], "crossing_mediated", "{mode}");
        let robots = &recorded[0]["payload"]["declarations"]["robots"];
        assert_eq!(robots["status"], 403, "{mode}: {robots}");
        assert!(robots["unreachable"].is_null(), "{mode}: {robots}");
        assert!(robots["unavailable"].is_null(), "{mode}: {robots}");
        assert!(
            robots["reading"]["group"].is_null(),
            "{mode}: no rules: {robots}"
        );
        assert_eq!(robots["outcome"], "allowed", "{mode}: {robots}");
    }
}

#[test]
fn a_429_or_503_with_nothing_held_refuses_in_every_mode_before_the_page() {
    for status in [429, 503] {
        for mode in MODES {
            let site = answering(move || Response::text(status, "not now"));
            let home = home_with_mode(mode);
            let page = site.url("/article");
            let response = fetch(home.path(), &page);
            assert_unreachable(
                mode,
                &site,
                &page,
                &format!("answered {status}"),
                &response,
                home.path(),
            );
            assert_eq!(
                crossings(home.path())[0]["payload"]["declarations"]["robots"]["status"],
                status,
                "{mode}"
            );
        }
    }
}

#[test]
fn a_429_or_503_is_ruled_by_the_stale_copy_held_in_every_mode() {
    for status in [429, 503] {
        for mode in MODES {
            let site = answering(move || Response::text(status, "not now"));
            let home = home_with_mode(mode);
            hold_stale_copy(home.path(), &site, 3);
            let private = site.url("/private/report");
            let open = site.url("/article");

            let refused = fetch(home.path(), &private);
            assert_eq!(
                refused["result"]["isError"], true,
                "{status} {mode}: {refused}"
            );
            let detail = text_of(&refused);
            assert!(
                detail.contains("`Disallow: /private/`")
                    && detail.contains("held because the file could not be read now")
                    && detail.contains(REFUSED),
                "{status} {mode}: {detail}"
            );
            let admitted = fetch(home.path(), &open);
            assert_eq!(
                admitted["result"]["isError"], false,
                "{status} {mode}: {admitted}"
            );
            assert_eq!(
                site.requests()
                    .into_iter()
                    .filter(|target| target != "/.well-known/content-telemetry.json")
                    .collect::<Vec<_>>(),
                ["/robots.txt", "/article"],
                "{status} {mode}: the failure is remembered, and only the allowed page is asked for"
            );

            let recorded = crossings(home.path());
            assert_eq!(recorded[0]["event"], "crossing_refused", "{status} {mode}");
            assert_eq!(recorded[1]["event"], "crossing_mediated", "{status} {mode}");
            for (crossing, outcome) in recorded.iter().zip(["refused", "allowed"]) {
                let robots = &crossing["payload"]["declarations"]["robots"];
                assert_eq!(robots["status"], status, "{status} {mode}: {robots}");
                assert_eq!(robots["unreachable"], true, "{status} {mode}: {robots}");
                assert_eq!(
                    robots["held_copy"]["status"], 200,
                    "{status} {mode}: {robots}"
                );
                assert_eq!(robots["outcome"], outcome, "{status} {mode}: {robots}");
            }
            // A failure never overwrites the answer it could not replace.
            let cached: Value = serde_json::from_slice(
                &std::fs::read(home.path().join("declarations/127.0.0.1.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(
                cached["robots_held"]["status"], 200,
                "{status} {mode}: {cached}"
            );
            assert_eq!(
                cached["robots"]["status"], status,
                "{status} {mode}: {cached}"
            );
        }
    }
}

/// `PROBE_TIMEOUT` is 5 seconds; the origin answers after 7, so the refusal
/// names the 5-second budget the file did not answer within. The modes run
/// side by side so the file waits for one timeout, not three.
#[test]
fn a_robots_file_that_does_not_answer_in_time_refuses_in_every_mode() {
    std::thread::scope(|scope| {
        for mode in MODES {
            scope.spawn(move || {
                let site = answering(|| {
                    std::thread::sleep(Duration::from_secs(7));
                    Response::text(200, "User-agent: *\nAllow: /\n")
                });
                let home = home_with_mode(mode);
                let page = site.url("/article");
                let response = fetch(home.path(), &page);
                assert_unreachable(
                    mode,
                    &site,
                    &page,
                    "did not answer within the 5s exchange budget",
                    &response,
                    home.path(),
                );
            });
        }
    });
}

/// RFC 9309 §2.5 lets a crawler stop parsing at a limit of at least 500 KiB;
/// it does not let it discard the file. A `Disallow` near the top of a
/// 600 KiB file refuses in every mode, the record says how much was read, and
/// a rule written past the limit is not read.
#[test]
fn an_oversized_robots_file_is_read_to_the_parsing_limit_in_every_mode() {
    let mut body = String::from("License: /license.xml\n\nUser-agent: *\nDisallow: /private/\n");
    while body.len() < 600 * 1024 {
        body.push_str("# a line of padding that a publisher's generator wrote\n");
    }
    body.push_str("Disallow: /late/\n");
    let size = body.len() as u64;
    let body: &'static str = Box::leak(body.into_boxed_str());
    for mode in MODES {
        let site = origin(body);
        let home = home_with_mode(mode);
        let page = site.url("/private/report");
        let response = fetch(home.path(), &page);
        assert_refused(
            mode,
            &site,
            &page,
            "*",
            "Disallow: /private/",
            &response,
            home.path(),
        );
        let detail = text_of(&response);
        assert!(
            detail.contains(&format!("of its {size} bytes")) && detail.contains("section 2.5"),
            "{mode}: {detail}"
        );
        let robots = &crossings(home.path())[0]["payload"]["declarations"]["robots"];
        assert_eq!(robots["truncated"]["size"], size, "{mode}: {robots}");
        let read = robots["truncated"]["read"].as_u64().expect("bytes read");
        assert!(read > 0 && read <= 512 * 1024, "{mode}: {read}");
        assert!(robots["unavailable"].is_null(), "{mode}: {robots}");
        assert_eq!(robots["outcome"], "refused", "{mode}: {robots}");

        // The rule past the limit is not read, so that page goes.
        let late = site.url("/late/page");
        let admitted = fetch(home.path(), &late);
        assert_eq!(admitted["result"]["isError"], false, "{mode}: {admitted}");
        assert!(
            site.requests().iter().any(|target| target == "/late/page"),
            "{mode}: {:?}",
            site.requests()
        );
    }
}

/// An origin whose `robots.txt` answers 301 to `localhost` on another
/// port, which serves `robots`, and the page text elsewhere; and the URL
/// the redirect names.
fn redirecting_to_localhost(robots: &'static str) -> (Origin, Origin, String) {
    let www = serving(move |_| Response::text(200, robots));
    let www_robots = format!("http://localhost:{}/robots.txt", www.handle.addr().port());
    let location = www_robots.clone();
    let site = serving(move |target| match target {
        "/robots.txt" => redirect(301, &location),
        _ => Response::text(200, "the page text"),
    });
    (site, www, www_robots)
}

/// The refusal a declined `robots.txt` redirect earns: the file unreachable,
/// the redirect and the reason named with RFC 9309 §2.3.1.2 cited, the
/// declined target on the record, the origin asked for nothing but
/// `robots.txt`, the redirect's target asked for nothing, and the refusal
/// pointing at the operator's policy, where the redirect can be admitted,
/// rather than at the source's file.
fn assert_redirect_declined(
    mode: &str,
    site: &Origin,
    www: &Origin,
    www_robots: &str,
    reason: &str,
    home: &Path,
) {
    let page = site.url("/article");
    let response = fetch(home, &page);
    assert_unreachable(
        mode,
        site,
        &page,
        &format!(
            "{} redirected to {www_robots}, which this edge does not follow: {reason}",
            site.url("/robots.txt")
        ),
        &response,
        home,
    );
    assert_eq!(site.requests(), ["/robots.txt"], "{mode}");
    assert!(www.requests().is_empty(), "{mode}: {:?}", www.requests());
    let detail = text_of(&response);
    assert!(
        detail.contains("section 2.3.1.2 on redirects"),
        "{mode}: {detail}"
    );
    assert!(
        detail.contains(&format!(
            "(operator policy in {}, which does not admit {www_robots}, where the source's \
             robots.txt redirected;",
            home.join("policy.json").display()
        )) && !detail.contains("(the source's robots.txt,"),
        "{mode}: {detail}"
    );
    let robots = &crossings(home)[0]["payload"]["declarations"]["robots"];
    assert_eq!(robots["declined_redirect"], www_robots, "{mode}: {robots}");
    assert!(robots["final_url"].is_null(), "{mode}: {robots}");
}

/// A `robots.txt` redirect to an address this edge does not mediate is
/// declined in every mode, so the file is unreachable and, with nothing
/// held, a complete disallow (RFC 9309 §2.3.1.2, §2.3.1.4). The operator
/// names only the origin's prefix as mediated, so `localhost` stands for an
/// address outside the floor.
#[test]
fn a_robots_redirect_to_an_address_the_edge_does_not_mediate_is_unreachable_in_every_mode() {
    for mode in MODES {
        let (site, www, www_robots) = redirecting_to_localhost("User-agent: *\nAllow: /\n");
        let home = home_with_policy(json!({
            "policy_mode": mode,
            "record_internal_prefixes": [site.url("/")],
        }));
        assert_redirect_declined(
            mode,
            &site,
            &www,
            &www_robots,
            &format!(
                "{www_robots} is a local or private address, which Common Measure does not mediate"
            ),
            home.path(),
        );
    }
}

/// The realistic case from the review: `example.com/robots.txt` answers 301
/// to `www.example.com/robots.txt`, and the operator's policy admits only
/// `example.com`. Here `127.0.0.1` stands for the admitted host and
/// `localhost` for the `www` host.
///
/// Under `strict` the policy refuses the `www` host, so the redirect is
/// declined and the file is unreachable: the page is refused and no page
/// request reaches the origin. Under `observe` and `prefer` the host
/// allowlist records a breach rather than refusing (`Ruling::breach`), so
/// the redirect is followed as RFC 9309 §2.3.1.2 expects, and the file it
/// lands on rules the page. The record names that file, and the crossing
/// that sent the request to the `www` host carries the host-policy breach;
/// a later crossing that reuses the cached copy sends nothing there and
/// carries none.
#[test]
fn a_robots_redirect_to_a_host_outside_the_allowlist_is_declined_under_strict() {
    let allow_only_site = |mode: &str| {
        home_with_policy(json!({
            "policy_mode": mode,
            "allow_private_hosts": true,
            "constraints": [{"kind": "allowed_source_host", "host": "127.0.0.1"}],
        }))
    };
    let (site, www, www_robots) = redirecting_to_localhost("User-agent: *\nAllow: /\n");
    let home = allow_only_site("strict");
    assert_redirect_declined(
        "strict",
        &site,
        &www,
        &www_robots,
        "Host localhost is outside the job's allowed-host list.",
        home.path(),
    );

    for mode in ["prefer", "observe"] {
        let (site, www, www_robots) =
            redirecting_to_localhost("User-agent: *\nDisallow: /private/\n");
        let home = allow_only_site(mode);
        let refused = fetch(home.path(), &site.url("/private/report"));
        assert_eq!(refused["result"]["isError"], true, "{mode}: {refused}");
        assert!(
            text_of(&refused).contains(&format!(
                "{} (redirected to {www_robots}) disallows",
                site.url("/robots.txt")
            )),
            "{mode}: {refused}"
        );
        assert!(
            text_of(&refused).contains("`Disallow: /private/`"),
            "{mode}: {refused}"
        );
        let admitted = fetch(home.path(), &site.url("/article"));
        assert_eq!(admitted["result"]["isError"], false, "{mode}: {admitted}");
        assert_eq!(www.requests(), ["/robots.txt"], "{mode}");

        let recorded = crossings(home.path());
        assert_eq!(recorded.len(), 2, "{mode}");
        let first = &recorded[0]["payload"];
        assert_eq!(first["declarations"]["robots"]["final_url"], www_robots);
        let breach = first["breach"].as_str().unwrap_or_default();
        assert!(
            breach.contains(&format!(
                "{} redirected to {www_robots}, which was requested for its rules: Host \
                 localhost is outside the job's allowed-host list.",
                site.url("/robots.txt")
            )),
            "{mode}: {first}"
        );
        let second = &recorded[1]["payload"];
        assert_eq!(second["declarations"]["robots"]["cache"], "reused");
        assert_eq!(second["declarations"]["robots"]["final_url"], www_robots);
        assert!(second["breach"].is_null(), "{mode}: {second}");
        assert!(
            !site
                .requests()
                .iter()
                .any(|target| target == "/private/report"),
            "{mode}: {:?}",
            site.requests()
        );
    }
}

/// The same redirect where the policy admits both hosts is followed, and the
/// file it lands on is the one that rules the page.
#[test]
fn a_robots_redirect_the_policy_admits_is_followed_and_its_file_rules() {
    let www = serving(|_| Response::text(200, "User-agent: *\nDisallow: /private/\n"));
    let location = format!("http://localhost:{}/robots.txt", www.handle.addr().port());
    let site = serving(move |target| match target {
        "/robots.txt" => redirect(301, &location),
        _ => Response::text(200, "the page text"),
    });
    let home = home_with_mode("observe");
    let refused = fetch(home.path(), &site.url("/private/report"));
    assert_eq!(refused["result"]["isError"], true, "{refused}");
    assert!(
        text_of(&refused).contains("`Disallow: /private/`"),
        "{refused}"
    );
    let admitted = fetch(home.path(), &site.url("/article"));
    assert_eq!(admitted["result"]["isError"], false, "{admitted}");
    assert_eq!(www.requests(), ["/robots.txt"]);
    assert!(
        !site
            .requests()
            .iter()
            .any(|target| target == "/private/report"),
        "{:?}",
        site.requests()
    );
}

/// RFC 9309 §2.3.1.2: a crawler follows at least five redirects for
/// `robots.txt`. Five are followed; a sixth is not, and the file is then
/// unreachable, the stricter of the two readings the RFC allows.
#[test]
fn a_robots_file_five_redirects_away_is_read_and_six_is_unreachable() {
    for (redirects, outcome) in [(5, "refused"), (6, "unreachable")] {
        let site = serving(move |target| {
            let hop = match target {
                "/robots.txt" => 0,
                other => match other.strip_prefix("/hop/").and_then(|n| n.parse().ok()) {
                    Some(n) => n,
                    None => return Response::text(200, "the page text"),
                },
            };
            if hop < redirects {
                redirect(302, &format!("/hop/{}", hop + 1))
            } else {
                Response::text(200, "User-agent: *\nDisallow: /private/\n")
            }
        });
        let home = home_with_mode("observe");
        let response = fetch(home.path(), &site.url("/private/report"));
        assert_eq!(
            response["result"]["isError"], true,
            "{redirects}: {response}"
        );
        let robots = &crossings(home.path())[0]["payload"]["declarations"]["robots"];
        assert_eq!(robots["outcome"], outcome, "{redirects}: {robots}");
        if outcome == "unreachable" {
            assert!(
                text_of(&response).contains("redirect limit exceeded"),
                "{response}"
            );
        }
        assert!(
            !site
                .requests()
                .iter()
                .any(|target| target == "/private/report"),
            "{redirects}: {:?}",
            site.requests()
        );
    }
}
