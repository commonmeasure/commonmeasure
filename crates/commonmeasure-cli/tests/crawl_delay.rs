//! `Crawl-delay` enforcement, driven through the real binary over loopback
//! origins: which group's delay is read, what the fetch did about it, and
//! what the record says. The delay binds in every policy mode (owner
//! decision, 14 September 2026), so the modes here vary and the outcome does
//! not.
//!
//! A test that waits one out uses two seconds, short enough to keep the file
//! quick and long enough that a loaded machine does not leave the delay
//! before the second fetch starts. A test that needs a refusal writes the
//! turn another request would have left, dated ahead, so the wait is longer
//! than the whole budget and no test waits a minute for it.

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use commonmeasure_http::{Response, Server, ServerHandle};
use serde_json::{Value, json};

/// One loopback origin: its `robots.txt`, and when it answered each page
/// request.
struct Origin {
    handle: ServerHandle,
    host: &'static str,
    seen: Arc<Mutex<Vec<(String, Instant)>>>,
}

impl Origin {
    fn url(&self, path: &str) -> String {
        format!("http://{}:{}{path}", self.host, self.handle.addr().port())
    }

    /// Every page request this origin answered, in order. `robots.txt` and
    /// the Content Telemetry manifest are discovery, not the crossing the
    /// delay governs.
    fn pages(&self) -> Vec<(String, Instant)> {
        self.requests()
            .into_iter()
            .filter(|(target, _)| {
                target != "/robots.txt"
                    && target != "/.well-known/content-telemetry.json"
                    && target != "/license.xml"
            })
            .collect()
    }

    /// Every request this origin answered, in order: what a publisher's own
    /// log holds, which cannot tell a probe from a page.
    fn requests(&self) -> Vec<(String, Instant)> {
        self.seen.lock().unwrap().clone()
    }

    fn first(&self, target: &str) -> Instant {
        self.requests()
            .into_iter()
            .find(|(asked, _)| asked == target)
            .unwrap_or_else(|| panic!("{target} was never asked for"))
            .1
    }
}

fn origin(host: &'static str, robots: &'static str, redirect_to: Option<String>) -> Origin {
    serve(host, robots, redirect_to, None)
}

/// An origin whose `robots.txt` names `/license.xml`, which it serves.
fn licensed_origin(robots: &'static str, licence: &'static str) -> Origin {
    serve("127.0.0.1", robots, None, Some(licence))
}

fn serve(
    host: &'static str,
    robots: &'static str,
    redirect_to: Option<String>,
    licence: Option<&'static str>,
) -> Origin {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |request| {
            log.lock()
                .unwrap()
                .push((request.target.clone(), Instant::now()));
            if request.target == "/robots.txt" {
                return Response::text(200, robots);
            }
            if let Some(licence) = licence
                && request.target == "/license.xml"
            {
                return Response::new(200, licence.as_bytes().to_vec());
            }
            if let Some(target) = &redirect_to
                && request.target.starts_with("/s/")
            {
                let mut response = Response::new(302, Vec::new());
                response.headers.set("Location", target);
                return response;
            }
            Response::text(200, "the page text")
        })
        .expect("spawn");
    Origin { handle, host, seen }
}

fn start(home: &Path, session: &str) -> Child {
    Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["mcp", "--host", "claude-code", "--session", session])
        .env("COMMONMEASURE_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the binary should start")
}

fn answers(child: Child) -> Vec<Value> {
    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success(), "the server should exit cleanly");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
        .collect()
}

fn ask(child: &mut Child, requests: &[Value]) {
    let stdin = child.stdin.as_mut().expect("stdin");
    for request in requests {
        writeln!(stdin, "{request}").expect("write request");
    }
}

/// One server process, asked for `requests` and closed.
fn converse(home: &Path, requests: &[Value]) -> Vec<Value> {
    let mut child = start(home, "test-session");
    ask(&mut child, requests);
    drop(child.stdin.take());
    answers(child)
}

/// As [`converse`], with the server started in `cwd`, which is what the
/// policy's scopes match.
fn converse_in(home: &Path, cwd: &Path, requests: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["mcp", "--host", "claude-code", "--session", "test-session"])
        .env("COMMONMEASURE_HOME", home)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the binary should start");
    ask(&mut child, requests);
    drop(child.stdin.take());
    answers(child)
}

fn fetch(id: u64, url: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": "context_fetch", "arguments": {"url": url}},
    })
}

fn text_of(response: &Value) -> String {
    response["result"]["content"][0]["text"]
        .as_str()
        .expect("a tool result carries text")
        .to_owned()
}

fn payload(response: &Value) -> Value {
    serde_json::from_str(&text_of(response)).expect("the payload is JSON")
}

fn crossings(home: &Path) -> Vec<Value> {
    let path = home.join("sessions/test-session.ndjson");
    let file = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    file.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("the log is NDJSON"))
        .filter(|record| {
            record["event"]
                .as_str()
                .is_some_and(|event| event.starts_with("crossing_"))
        })
        .collect()
}

/// Write the turn an earlier request left, dated `at`. This is the store's
/// own file under the operator home, which is how a second server, or the
/// same edge before a restart, leaves a turn behind.
fn seed_turn(home: &Path, host: &str, at: chrono::DateTime<chrono::Utc>) {
    let path = commonmeasure_harness::crawl_delay::CrawlDelayStore::open(home).path_of(host);
    std::fs::create_dir_all(path.parent().expect("a directory")).expect("the store directory");
    std::fs::write(
        &path,
        serde_json::to_vec(&json!({"host": host, "at": at})).expect("a record"),
    )
    .expect("the record");
}

fn home_with_mode(mode: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        home.path().join("policy.json"),
        format!(r#"{{"policy_mode":"{mode}","allow_private_hosts":true}}"#),
    )
    .expect("policy");
    home
}

const NAMED_TWO_SECONDS: &str = "\
User-agent: *
Crawl-delay: 30

User-agent: CommonMeasureBot
Allow: /
Crawl-delay: 2
";

const WILDCARD_TWO_SECONDS: &str = "User-agent: *\nAllow: /\nCrawl-delay: 2\n";

const NAMED_WITHOUT_A_DELAY: &str = "\
User-agent: *
Crawl-delay: 30

User-agent: CommonMeasureBot
Allow: /
";

const UNREADABLE: &str = "User-agent: *\nAllow: /\nCrawl-delay: soon\n";

const THE_BOUND: &str = "User-agent: *\nAllow: /\nCrawl-delay: 60\n";

const NO_WAIT: &str = "User-agent: *\nAllow: /\nCrawl-delay: 0\n";

const OVER_THE_BOUND: &str = "User-agent: *\nAllow: /\nCrawl-delay: 3600\n";

const NO_DELAY: &str = "User-agent: *\nAllow: /\n";

#[test]
fn the_delay_of_the_group_naming_the_product_token_is_waited_out() {
    let site = origin("127.0.0.1", NAMED_TWO_SECONDS, None);
    let home = home_with_mode("observe");
    let article = site.url("/article");

    let started = Instant::now();
    let responses = converse(home.path(), &[fetch(1, &article), fetch(2, &article)]);
    let elapsed = started.elapsed();
    assert_eq!(responses[1]["result"]["isError"], false, "{}", responses[1]);
    assert!(
        elapsed >= Duration::from_millis(1_900),
        "two fetches inside a 2s delay took {elapsed:?}"
    );
    let pages = site.pages();
    assert_eq!(pages.len(), 2);
    // The turn is taken just before the request is signed and sent, so the
    // gap the origin sees is the delay less the few milliseconds between the
    // two.
    assert!(
        pages[1].1.duration_since(pages[0].1) >= Duration::from_millis(1_900),
        "the origin was asked twice inside its delay"
    );

    let shown = payload(&responses[1])["declarations"]["robots"].clone();
    assert_eq!(shown["group"], "CommonMeasureBot");
    assert_eq!(shown["crawl_delay"]["value"], "2");
    assert_eq!(shown["crawl_delay"]["delay_ms"], 2_000);
    assert_eq!(shown["crawl_delay"]["honoured_ms"], 2_000);
    assert_eq!(shown["crawl_delay"]["capped"], false);
    assert_eq!(shown["delay"]["outcome"], "waited");
    assert_eq!(shown["delay"]["host"], "127.0.0.1");
    assert!(
        shown["delay"]["wait_ms"].as_u64().expect("a wait") <= 2_000,
        "{shown}"
    );

    let recorded = crossings(home.path());
    assert_eq!(recorded.len(), 2);
    let first = &recorded[0]["payload"]["declarations"]["robots"];
    // The first crossing's free turn goes to the manifest probe, which would
    // never get one after the page, so the page itself waits behind it.
    assert_eq!(first["delay"]["outcome"], "waited");
    assert_eq!(first["delay"]["delay_ms"], 2_000);
    let second = &recorded[1]["payload"]["declarations"]["robots"];
    assert_eq!(second["delay"]["outcome"], "waited");
    assert_eq!(second["reading"]["crawl_delay"]["delay_ms"], 2_000);
}

#[test]
fn the_wildcard_group_supplies_the_delay_where_no_group_names_the_token() {
    let site = origin("127.0.0.1", WILDCARD_TWO_SECONDS, None);
    let home = home_with_mode("prefer");
    let article = site.url("/article");

    let responses = converse(home.path(), &[fetch(1, &article), fetch(2, &article)]);
    assert_eq!(responses[1]["result"]["isError"], false, "{}", responses[1]);
    let shown = payload(&responses[1])["declarations"]["robots"].clone();
    assert_eq!(shown["group"], "*");
    assert_eq!(shown["group_is_wildcard"], true);
    assert_eq!(shown["crawl_delay"]["delay_ms"], 2_000);
    assert_eq!(shown["delay"]["outcome"], "waited");
}

#[test]
fn a_group_naming_the_token_without_a_delay_stands_over_the_wildcard_groups() {
    let site = origin("127.0.0.1", NAMED_WITHOUT_A_DELAY, None);
    let home = home_with_mode("observe");
    let article = site.url("/article");

    let started = Instant::now();
    let responses = converse(home.path(), &[fetch(1, &article), fetch(2, &article)]);
    assert_eq!(responses[1]["result"]["isError"], false, "{}", responses[1]);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the `*` group's 30s delay is not this fetcher's"
    );
    let shown = payload(&responses[1])["declarations"]["robots"].clone();
    assert_eq!(shown["group"], "CommonMeasureBot");
    assert!(shown["crawl_delay"].is_null(), "{shown}");
    assert!(shown["delay"].is_null(), "{shown}");
    assert_eq!(site.pages().len(), 2);
}

#[test]
fn an_unreadable_delay_is_recorded_and_ignored() {
    let site = origin("127.0.0.1", UNREADABLE, None);
    let home = home_with_mode("observe");
    let article = site.url("/article");

    let responses = converse(home.path(), &[fetch(1, &article), fetch(2, &article)]);
    assert_eq!(responses[1]["result"]["isError"], false, "{}", responses[1]);
    let shown = payload(&responses[1])["declarations"]["robots"].clone();
    assert_eq!(shown["crawl_delay"]["unreadable"][0], "soon");
    assert!(shown["crawl_delay"]["delay_ms"].is_null(), "{shown}");
    assert!(shown["delay"].is_null(), "{shown}");
    assert_eq!(site.pages().len(), 2, "nothing was withheld");
}

#[test]
fn a_delay_beyond_the_wait_budget_refuses_the_fetch_naming_the_host_and_the_delay() {
    let site = origin("127.0.0.1", THE_BOUND, None);
    let home = home_with_mode("strict");
    let article = site.url("/article");
    // Another request took its turn half a minute ahead, so the wait is
    // longer than the whole budget of this fetch.
    seed_turn(
        home.path(),
        "127.0.0.1",
        chrono::Utc::now() + chrono::Duration::seconds(30),
    );

    let started = Instant::now();
    let responses = converse(home.path(), &[fetch(1, &article)]);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "a refusal waits for nothing"
    );
    assert_eq!(responses[0]["result"]["isError"], true, "{}", responses[0]);
    let detail = text_of(&responses[0]);
    for needed in [
        "refused before the crossing",
        "Crawl-delay: 60",
        "binds in every policy mode",
        "127.0.0.1",
        "may be sent",
        "this fetch may still wait",
        "Nothing was requested.",
    ] {
        assert!(detail.contains(needed), "missing {needed:?} in {detail}");
    }
    assert_eq!(site.pages().len(), 0, "nothing was requested");

    let recorded = crossings(home.path());
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0]["event"], "crossing_refused");
    let payload = &recorded[0]["payload"];
    assert_eq!(payload["grounded"], false);
    assert!(payload["http_status"].is_null(), "nothing was requested");
    let robots = &payload["declarations"]["robots"];
    assert_eq!(robots["delay"]["outcome"], "refused");
    assert_eq!(robots["delay"]["delay_ms"], 60_000);
    let wait = robots["delay"]["wait_ms"].as_u64().expect("a wait");
    let budget = robots["delay"]["budget_ms"].as_u64().expect("a budget");
    assert!((80_000..=90_000).contains(&wait), "{robots}");
    assert_eq!(budget, 60_000, "{robots}");
    assert!(wait > budget, "{robots}");
    assert!(robots["delay"]["next_at"].is_string(), "{robots}");
}

#[test]
fn a_delay_over_the_bound_is_kept_at_the_bound_and_recorded_as_capped() {
    let site = origin("127.0.0.1", OVER_THE_BOUND, None);
    let home = home_with_mode("observe");
    let article = site.url("/article");
    seed_turn(
        home.path(),
        "127.0.0.1",
        chrono::Utc::now() + chrono::Duration::seconds(30),
    );

    let responses = converse(home.path(), &[fetch(1, &article)]);
    assert_eq!(responses[0]["result"]["isError"], true, "{}", responses[0]);
    let detail = text_of(&responses[0]);
    assert!(detail.contains("Crawl-delay: 3600"), "{detail}");
    assert!(
        detail.contains("kept at this edge's bound of 60s"),
        "{detail}"
    );
    assert_eq!(site.pages().len(), 0);

    let robots = &crossings(home.path())[0]["payload"]["declarations"]["robots"];
    assert_eq!(robots["reading"]["crawl_delay"]["delay_ms"], 3_600_000);
    assert_eq!(robots["reading"]["crawl_delay"]["honoured_ms"], 60_000);
    assert_eq!(robots["reading"]["crawl_delay"]["capped"], true);
    assert_eq!(robots["delay"]["delay_ms"], 60_000);
}

#[test]
fn the_delay_is_kept_across_two_server_processes() {
    let site = origin("127.0.0.1", WILDCARD_TWO_SECONDS, None);
    let home = home_with_mode("observe");
    let article = site.url("/article");

    let first = converse(home.path(), &[fetch(1, &article)]);
    assert_eq!(first[0]["result"]["isError"], false, "{}", first[0]);
    let second = converse(home.path(), &[fetch(1, &article)]);
    assert_eq!(second[0]["result"]["isError"], false, "{}", second[0]);
    assert_eq!(
        payload(&second[0])["declarations"]["robots"]["delay"]["outcome"],
        "waited",
        "a restart does not forget the last request"
    );
    let pages = site.pages();
    assert_eq!(pages.len(), 2);
    assert!(
        pages[1].1.duration_since(pages[0].1) >= Duration::from_millis(1_900),
        "the second process fired inside the delay"
    );
}

#[test]
fn two_servers_asking_at_once_do_not_both_fire_inside_the_delay() {
    let site = origin("127.0.0.1", WILDCARD_TWO_SECONDS, None);
    let home = home_with_mode("observe");
    let article = site.url("/article");

    // Each server writes its own session log; the record they share is the
    // last request to the host.
    let mut left = start(home.path(), "left");
    let mut right = start(home.path(), "right");
    ask(&mut left, &[fetch(1, &article)]);
    ask(&mut right, &[fetch(1, &article)]);
    drop(left.stdin.take());
    drop(right.stdin.take());
    for answer in [answers(left), answers(right)] {
        assert_eq!(answer[0]["result"]["isError"], false, "{}", answer[0]);
    }

    let pages = site.pages();
    assert_eq!(pages.len(), 2);
    assert!(
        pages[1].1.duration_since(pages[0].1) >= Duration::from_millis(1_900),
        "two servers fired inside the delay"
    );
}

#[test]
fn a_redirect_hop_takes_its_own_hosts_turn() {
    // The destination is addressed as `localhost` and the shortener as
    // `127.0.0.1`, so each has its own `robots.txt` and its own delay.
    let destination = origin("localhost", THE_BOUND, None);
    let landing = destination.url("/landing");
    let shortener = origin("127.0.0.1", NO_DELAY, Some(landing.clone()));
    let home = home_with_mode("observe");
    seed_turn(
        home.path(),
        "localhost",
        chrono::Utc::now() + chrono::Duration::seconds(30),
    );

    let responses = converse(home.path(), &[fetch(1, &shortener.url("/s/abc"))]);
    assert_eq!(responses[0]["result"]["isError"], true, "{}", responses[0]);
    let detail = text_of(&responses[0]);
    assert!(detail.contains("Crawl-delay: 60"), "{detail}");
    assert!(detail.contains("localhost"), "{detail}");
    assert_eq!(
        destination.pages().len(),
        0,
        "the hop into the delay was not requested"
    );
    assert_eq!(shortener.pages().len(), 1, "the shortener answered once");

    let recorded = crossings(home.path());
    assert_eq!(recorded[0]["event"], "crossing_refused");
    let payload = &recorded[0]["payload"];
    assert_eq!(payload["url"], landing, "the refused hop is on the record");
    assert_eq!(
        payload["declarations"]["robots"]["delay"]["outcome"],
        "refused"
    );
}

#[test]
fn a_zero_delay_paces_nothing() {
    let site = origin("127.0.0.1", NO_WAIT, None);
    let home = home_with_mode("observe");
    let article = site.url("/article");

    let started = Instant::now();
    let responses = converse(home.path(), &[fetch(1, &article), fetch(2, &article)]);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "nothing waited"
    );
    assert_eq!(responses[1]["result"]["isError"], false, "{}", responses[1]);
    let shown = payload(&responses[1])["declarations"]["robots"].clone();
    assert_eq!(shown["crawl_delay"]["delay_ms"], 0);
    assert_eq!(shown["crawl_delay"]["honoured_ms"], 0);
    assert!(shown["delay"].is_null(), "no turn is taken: {shown}");
    assert_eq!(site.pages().len(), 2);
}

/// A `robots.txt` answering 503 with no earlier answer held is unreachable
/// (RFC 9309 §2.3.1.4): the crossing is refused before the page, takes no
/// turn and waits for nothing, and the failure is remembered for the
/// failure age so the second crossing does not ask again.
#[test]
fn a_robots_file_that_cannot_be_reached_refuses_paces_nothing_and_says_so() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |request| {
            log.lock()
                .unwrap()
                .push((request.target.clone(), Instant::now()));
            if request.target == "/robots.txt" {
                return Response::text(503, "busy");
            }
            Response::text(200, "the page text")
        })
        .expect("spawn");
    let site = Origin {
        handle,
        host: "127.0.0.1",
        seen,
    };
    let home = home_with_mode("observe");
    let article = site.url("/article");

    let started = Instant::now();
    let responses = converse(home.path(), &[fetch(1, &article), fetch(2, &article)]);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "an unreachable file states no delay"
    );
    for response in &responses {
        assert_eq!(response["result"]["isError"], true, "{response}");
        assert!(text_of(response).contains("answered 503"), "{response}");
    }
    let recorded = crossings(home.path());
    assert_eq!(recorded.len(), 2);
    for crossing in &recorded {
        let robots = &crossing["payload"]["declarations"]["robots"];
        assert_eq!(crossing["event"], "crossing_refused");
        assert_eq!(robots["status"], 503, "{robots}");
        assert_eq!(robots["outcome"], "unreachable", "{robots}");
        assert!(robots["delay"].is_null(), "{robots}");
    }
    assert_eq!(
        recorded[1]["payload"]["declarations"]["robots"]["cache"],
        "reused"
    );
    assert!(site.pages().is_empty(), "{:?}", site.pages());
    assert_eq!(site.requests().len(), 1, "robots.txt was asked once");
}

#[test]
fn a_store_that_cannot_be_kept_refuses_and_names_the_remedy() {
    let site = origin("127.0.0.1", WILDCARD_TWO_SECONDS, None);
    let home = home_with_mode("observe");
    // A file where the store's directory must be: no turn can be kept, and
    // a delay that cannot be kept is not dropped (`docs/FAIL-POLICY.md` §5).
    std::fs::write(home.path().join("crawl-delay"), b"in the way").expect("the file");

    let responses = converse(home.path(), &[fetch(1, &site.url("/article"))]);
    assert_eq!(responses[0]["result"]["isError"], true, "{}", responses[0]);
    let detail = text_of(&responses[0]);
    for needed in [
        "Crawl-delay: 2",
        "could not be kept",
        "remove that directory",
        "Nothing was requested.",
    ] {
        assert!(detail.contains(needed), "missing {needed:?} in {detail}");
    }
    assert_eq!(site.pages().len(), 0);
    let robots = &crossings(home.path())[0]["payload"]["declarations"]["robots"];
    assert_eq!(robots["delay"]["outcome"], "unavailable");
    assert!(
        robots["delay"]["unavailable"]
            .as_str()
            .is_some_and(|reason| reason.contains("crawl-delay")),
        "{robots}"
    );
}

#[test]
fn a_same_host_redirect_takes_two_turns_and_the_second_has_less_budget() {
    // Both origins answer as `127.0.0.1`, so they are one host and one pace,
    // which is what a publisher behind two ports sees.
    let destination = origin("127.0.0.1", WILDCARD_TWO_SECONDS, None);
    let landing = destination.url("/landing");
    let shortener = origin("127.0.0.1", WILDCARD_TWO_SECONDS, Some(landing.clone()));
    let home = home_with_mode("observe");
    // A turn already taken, dated far enough ahead that starting the server
    // does not leave the delay behind: the first hop waits, and the second
    // is left less of the budget than the first had.
    seed_turn(
        home.path(),
        "127.0.0.1",
        chrono::Utc::now() + chrono::Duration::seconds(2),
    );

    let responses = converse(home.path(), &[fetch(1, &shortener.url("/s/abc"))]);
    assert_eq!(responses[0]["result"]["isError"], false, "{}", responses[0]);
    let payload = &crossings(home.path())[0]["payload"];
    let first = &payload["declarations"]["redirects"][0]["delay"];
    let second = &payload["declarations"]["robots"]["delay"];
    assert_eq!(first["outcome"], "waited", "{first}");
    assert_eq!(second["outcome"], "waited", "{second}");
    assert_eq!(first["budget_ms"], 60_000, "the first hop has it all");
    let left = second["budget_ms"].as_u64().expect("a budget");
    assert!(
        left < 59_000,
        "the wait of the first hop is not charged to the call: {second}"
    );
    assert!(
        shortener.pages()[0]
            .1
            .duration_since(shortener.first("/robots.txt"))
            >= Duration::from_millis(1_900),
        "the first hop did not wait out the turn already taken"
    );
    assert!(
        destination.pages()[0]
            .1
            .duration_since(shortener.pages()[0].1)
            >= Duration::from_millis(1_900),
        "the two hops fired inside the delay"
    );
}

#[test]
fn the_manifest_takes_the_crossings_free_turn_the_page_waits_behind_it_and_robots_txt_takes_none() {
    let site = origin("127.0.0.1", WILDCARD_TWO_SECONDS, None);
    let home = home_with_mode("observe");

    let responses = converse(home.path(), &[fetch(1, &site.url("/article"))]);
    assert_eq!(responses[0]["result"]["isError"], false, "{}", responses[0]);
    let robots = site.first("/robots.txt");
    let manifest = site.first("/.well-known/content-telemetry.json");
    let page = site.pages()[0].1;
    // `robots.txt` is exempt: the delay cannot be read without it, so it is
    // asked for at once and takes no turn.
    assert!(
        manifest.duration_since(robots) < Duration::from_millis(1_900),
        "the manifest waited for a turn robots.txt had not taken"
    );
    // The manifest is an ordinary request to the same host, so it takes a
    // turn — the free one, before the page, because after the page it would
    // need a whole delay from what the page left. The page waits behind it.
    assert!(
        page > manifest,
        "the manifest was not asked at the crossing's first free turn"
    );
    assert!(
        page.duration_since(manifest) >= Duration::from_millis(1_900),
        "the page did not wait for the turn the manifest probe took"
    );
    let delay = &crossings(home.path())[0]["payload"]["declarations"]["robots"]["delay"];
    assert_eq!(delay["outcome"], "waited", "{delay}");
}

/// A licence whose AI-input permission carries a telemetry reporting demand,
/// which binds in every policy mode.
const REPORTING_LICENCE: &str = r#"<rsl xmlns="https://rslstandard.org/rsl">
  <content url="/"><license>
    <permits type="usage">ai-input</permits>
    <payment type="attribution"/>
    <reporting type="telemetry" profile="https://contenttelemetry.org/profiles/spur"
               endpoint="https://telemetry.example.com/v1/events">
      <![CDATA[{"conformance_level": "grounding", "privacy_level": "minimal"}]]>
    </reporting>
  </license></content></rsl>"#;

const LICENSED_TWO_SECONDS: &str =
    "License: /license.xml\nUser-agent: *\nAllow: /\nCrawl-delay: 2\n";

const LICENSED_THIRTY_SECONDS: &str =
    "License: /license.xml\nUser-agent: *\nAllow: /\nCrawl-delay: 30\n";

/// A page is never admitted under a licence the edge has not read. On a host
/// with a delay, a licence with no current reading takes the host's next
/// turn and the page the turn after it, so the terms, and the reporting
/// demand among them, are ruled on before the page is asked for.
#[test]
fn a_licence_with_no_current_reading_is_read_before_the_page_and_its_demand_ruled_on() {
    // Unmet: nothing clears telemetry egress and no receiver is configured,
    // so the demand refuses the crossing once the licence has been read, and
    // the page is never asked for.
    let site = licensed_origin(LICENSED_TWO_SECONDS, REPORTING_LICENCE);
    let home = home_with_mode("strict");
    let responses = converse(home.path(), &[fetch(1, &site.url("/article"))]);
    assert_eq!(responses[0]["result"]["isError"], true, "{}", responses[0]);
    let detail = text_of(&responses[0]);
    assert!(detail.contains("requires telemetry reporting"), "{detail}");
    let asked: Vec<String> = site.requests().into_iter().map(|(t, _)| t).collect();
    assert_eq!(asked, ["/robots.txt", "/license.xml"], "{asked:?}");
    let recorded = crossings(home.path());
    let declarations = &recorded[0]["payload"]["declarations"];
    assert_eq!(declarations["licences"][0]["cache"], "fetched");
    assert_eq!(declarations["reporting"]["met"], false);
    assert!(
        declarations["robots"]["delay"].is_null(),
        "the page's turn is not taken for a crossing its licence refused: {declarations}"
    );

    // Met: the scope clears egress and a receiver is named. The licence is
    // read first, the page waits a whole delay behind it, and a second page
    // on the same host reuses the licence and takes one turn only.
    let site = licensed_origin(LICENSED_TWO_SECONDS, REPORTING_LICENCE);
    let home = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        home.path().join("policy.json"),
        r#"{"policy_mode":"strict","allow_private_hosts":true,
            "scopes":[{"match":"reporting-cleared","engagement":"research","allow_telemetry_egress":true}]}"#,
    )
    .expect("policy");
    std::fs::write(
        home.path().join("relay.json"),
        r#"{"receiver":"http://127.0.0.1:9/telemetry"}"#,
    )
    .expect("relay");
    let workspace = tempfile::tempdir().expect("tempdir");
    let cleared = workspace.path().join("reporting-cleared");
    std::fs::create_dir_all(&cleared).expect("workspace");
    let responses = converse_in(
        home.path(),
        &cleared,
        &[
            fetch(1, &site.url("/article")),
            fetch(2, &site.url("/other")),
        ],
    );
    for response in &responses {
        assert_eq!(response["result"]["isError"], false, "{response}");
    }
    let licence_at = site.first("/license.xml");
    let article_at = site.first("/article");
    let other_at = site.first("/other");
    assert!(
        article_at.duration_since(licence_at) >= Duration::from_millis(1_900),
        "the page was asked inside the licence's delay"
    );
    assert!(
        other_at.duration_since(article_at) >= Duration::from_millis(1_900),
        "the second page was asked inside the first page's delay"
    );
    let licence_asks = site
        .requests()
        .into_iter()
        .filter(|(target, _)| target == "/license.xml")
        .count();
    assert_eq!(licence_asks, 1, "a current reading is reused");
    let recorded = crossings(home.path());
    assert_eq!(recorded.len(), 2);
    let first = &recorded[0]["payload"]["declarations"];
    assert_eq!(first["licences"][0]["cache"], "fetched");
    assert_eq!(first["reporting"]["met"], true);
    assert_eq!(first["robots"]["delay"]["outcome"], "waited");
    let second = &recorded[1]["payload"]["declarations"];
    assert_eq!(second["licences"][0]["cache"], "reused");
    assert_eq!(second["reporting"]["met"], true);
}

/// Where the licence's turn and then the page's do not fit what the call may
/// still wait, the crossing is refused before anything is sent: the licence
/// is not read only to have the page refused after it.
#[test]
fn licence_then_page_beyond_the_budget_is_refused_before_any_request() {
    let site = licensed_origin(LICENSED_THIRTY_SECONDS, REPORTING_LICENCE);
    let home = home_with_mode("observe");
    let article = site.url("/article");
    // `robots.txt` is already in the cache, so the origin's log holds every
    // request this crossing makes.
    let now = chrono::Utc::now();
    std::fs::create_dir_all(home.path().join("declarations")).expect("cache");
    std::fs::write(
        home.path().join("declarations/127.0.0.1.json"),
        serde_json::to_vec(&json!({"robots": {
            "url": site.url("/robots.txt"),
            "fetched_at": now,
            "expires_at": now + chrono::Duration::hours(1),
            "final_url": site.url("/robots.txt"),
            "status": 200,
            "body": LICENSED_THIRTY_SECONDS,
        }}))
        .expect("a record"),
    )
    .expect("the cached robots.txt");
    // Another request's turn five seconds ahead: the page alone would wait
    // 35 seconds, which fits; the licence first and the page 30 seconds
    // after it is 65, which does not.
    seed_turn(home.path(), "127.0.0.1", now + chrono::Duration::seconds(5));

    let started = Instant::now();
    let responses = converse(home.path(), &[fetch(1, &article)]);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "a refusal waits for nothing"
    );
    assert_eq!(responses[0]["result"]["isError"], true, "{}", responses[0]);
    let detail = text_of(&responses[0]);
    for needed in [
        "refused before the crossing",
        "Crawl-delay: 30",
        "/license.xml has not been read",
        "the page the turn after it",
        "Nothing was requested.",
    ] {
        assert!(detail.contains(needed), "missing {needed:?} in {detail}");
    }
    assert!(
        site.requests().is_empty(),
        "the origin was asked: {:?}",
        site.requests()
    );
    let recorded = crossings(home.path());
    assert_eq!(recorded[0]["event"], "crossing_refused");
    let declarations = &recorded[0]["payload"]["declarations"];
    let delay = &declarations["robots"]["delay"];
    assert_eq!(delay["outcome"], "refused");
    assert!(
        delay["licence_first"]
            .as_str()
            .is_some_and(|licence| licence.ends_with("/license.xml")),
        "{delay}"
    );
    let wait = delay["wait_ms"].as_u64().expect("a wait");
    assert!((60_001..=66_000).contains(&wait), "{delay}");
    assert_eq!(declarations["licences"][0]["cache"], "not_asked");
    assert_eq!(declarations["licences"][0]["unread"], true);
}

/// An origin whose `robots.txt` names `/license.xml`, which answers
/// `status` with `body`, and whose pages answer 200.
fn licence_answering(status: u16, body: &'static str) -> Origin {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |request| {
            log.lock()
                .unwrap()
                .push((request.target.clone(), Instant::now()));
            match request.target.as_str() {
                "/robots.txt" => {
                    Response::text(200, "License: /license.xml\nUser-agent: *\nAllow: /\n")
                }
                "/license.xml" => Response::new(status, body.as_bytes().to_vec()),
                _ => Response::text(200, "the page text"),
            }
        })
        .expect("spawn");
    Origin {
        handle,
        host: "127.0.0.1",
        seen,
    }
}

/// A licence that exists and cannot be read has unknown terms, not none:
/// the page is not asked for in any mode, and the record says why. A body
/// that is not RSL, a 403 (the document is withheld) and a 503 are all
/// that case (owner decision, 22 September 2026).
#[test]
fn a_licence_that_cannot_be_read_admits_nothing_in_observe_and_strict() {
    for (status, body) in [
        (200, "this is not a licence document"),
        (403, "forbidden"),
        (503, "busy"),
    ] {
        for mode in ["observe", "strict"] {
            let site = licence_answering(status, body);
            let home = home_with_mode(mode);
            let responses = converse(home.path(), &[fetch(1, &site.url("/article"))]);
            let case = format!("{status} in {mode}");
            assert_eq!(
                responses[0]["result"]["isError"], true,
                "{case}: {}",
                responses[0]
            );
            let detail = text_of(&responses[0]);
            assert!(
                detail.contains("refused before the crossing"),
                "{case}: {detail}"
            );
            assert!(detail.contains("could not be read"), "{case}: {detail}");
            assert!(detail.contains("in any policy mode"), "{case}: {detail}");
            assert_eq!(site.pages().len(), 0, "{case}: the page was asked for");
            let recorded = crossings(home.path());
            let licence = &recorded[0]["payload"]["declarations"]["licences"][0];
            assert_eq!(licence["cache"], "fetched", "{case}");
            assert_eq!(licence["unread"], true, "{case}: {licence}");
            assert!(licence.get("missing").is_none(), "{case}: {licence}");
        }
    }
}

/// A licence the publisher named that answers 404 or 410 is missing, not
/// withheld: the crossing proceeds in every mode with no licence terms, and
/// the record and the tool result name the URL, the status and the gap
/// (owner decision, 22 September 2026).
#[test]
fn a_missing_licence_404_or_410_proceeds_with_the_gap_recorded_in_observe_and_strict() {
    for status in [404, 410] {
        for mode in ["observe", "strict"] {
            let site = licence_answering(status, "gone");
            let home = home_with_mode(mode);
            let responses = converse(home.path(), &[fetch(1, &site.url("/article"))]);
            let case = format!("{status} in {mode}");
            assert_ne!(
                responses[0]["result"]["isError"], true,
                "{case}: {}",
                responses[0]
            );
            assert_eq!(site.pages().len(), 1, "{case}: the page was not asked for");
            let result = payload(&responses[0]);
            assert_eq!(result["content"], "the page text", "{case}");
            let licence = &result["declarations"]["licences"][0];
            assert_eq!(licence["status"], status, "{case}: {licence}");
            assert_eq!(
                licence["missing"]["reason"], "evidence_missing",
                "{case}: {licence}"
            );
            let detail = licence["missing"]["detail"].as_str().unwrap_or_default();
            assert!(
                detail.contains(&site.url("/license.xml")),
                "{case}: {detail}"
            );
            assert!(
                detail.contains(&format!("answered {status}")),
                "{case}: {detail}"
            );
            assert!(detail.contains("is missing"), "{case}: {detail}");
            assert!(
                result["declarations"]["licences"][0]["reporting"].is_null(),
                "{case}"
            );
            let recorded = crossings(home.path());
            let crossing = &recorded[0];
            assert_eq!(crossing["event"], "crossing_mediated", "{case}: {crossing}");
            let licence = &crossing["payload"]["declarations"]["licences"][0];
            assert_eq!(licence["status"], status, "{case}: {licence}");
            assert!(licence.get("unread").is_none(), "{case}: {licence}");
            assert_eq!(
                licence["missing"]["reason"], "evidence_missing",
                "{case}: {licence}"
            );
            assert!(licence.get("terms").is_none(), "{case}: {licence}");
        }
    }
}

/// Write `robots` (and `licence`, where given) into the declaration cache as
/// current readings, so the origin's log holds every request the crossing
/// makes.
fn cache_declarations(home: &Path, site: &Origin, robots: &str, licence: Option<&str>) {
    let now = chrono::Utc::now();
    let probe = |path: &str, body: &str| {
        json!({
            "url": site.url(path),
            "fetched_at": now,
            "expires_at": now + chrono::Duration::hours(1),
            "final_url": site.url(path),
            "status": 200,
            "body": body,
        })
    };
    let mut record = json!({"robots": probe("/robots.txt", robots)});
    if let Some(licence) = licence {
        record["licences"] = json!({ site.url("/license.xml"): probe("/license.xml", licence) });
    }
    std::fs::create_dir_all(home.join("declarations")).expect("cache");
    std::fs::write(
        home.join("declarations/127.0.0.1.json"),
        serde_json::to_vec(&record).expect("a record"),
    )
    .expect("the cached declarations");
}

/// The breach sentence observe carries for a `Content-Signal` that
/// disallows AI input. A `Disallow` would refuse before any turn in every
/// mode (WP-29), so these tests carry a preference instead.
const SIGNAL_CARRIED: &str = "The source disallows AI input";
/// The host constraint's breach sentence.
const HOST_CARRIED: &str = "allowed-host";

const SIGNALLED_LICENSED_THIRTY_SECONDS: &str = "License: /license.xml\nUser-agent: *\nAllow: /\n\
     Content-Signal: ai-input=no\nCrawl-delay: 30\n";

/// Catches a refusal on the licence's turn that drops what observe was
/// carrying: the host's breach and the `Content-Signal` the robots file
/// states are both kept, each once, and nothing is sent.
#[test]
fn licence_then_page_over_budget_keeps_the_signal_and_host_breaches_in_observe() {
    let site = licensed_origin(SIGNALLED_LICENSED_THIRTY_SECONDS, REPORTING_LICENCE);
    let home = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        home.path().join("policy.json"),
        r#"{"policy_mode":"observe","allow_private_hosts":true,
            "constraints":[{"kind":"allowed_source_host","host":"www.gov.uk"}]}"#,
    )
    .expect("policy");
    cache_declarations(home.path(), &site, SIGNALLED_LICENSED_THIRTY_SECONDS, None);
    seed_turn(
        home.path(),
        "127.0.0.1",
        chrono::Utc::now() + chrono::Duration::seconds(5),
    );

    let responses = converse(home.path(), &[fetch(1, &site.url("/article"))]);
    assert_eq!(responses[0]["result"]["isError"], true, "{}", responses[0]);
    assert!(
        site.requests().is_empty(),
        "the origin was asked: {:?}",
        site.requests()
    );
    let recorded = crossings(home.path());
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0]["event"], "crossing_refused");
    let payload = &recorded[0]["payload"];
    assert_eq!(
        payload["declarations"]["robots"]["delay"]["outcome"],
        "refused"
    );
    assert!(
        payload["declarations"]["robots"]["delay"]["licence_first"].is_string(),
        "refused on the licence's turn: {payload}"
    );
    let breach = payload["breach"].as_str().expect("the breaches are kept");
    assert_eq!(breach.matches(SIGNAL_CARRIED).count(), 1, "{breach}");
    assert!(breach.contains("Content-Signal"), "{breach}");
    assert_eq!(breach.matches(HOST_CARRIED).count(), 1, "{breach}");
    assert!(payload["allowance"].is_null(), "{payload}");
}

/// A licence with a monetary payment term the allowance cannot cover.
const PRICED_LICENCE: &str = r#"<rsl xmlns="https://rslstandard.org/rsl">
  <content url="/"><license>
    <permits type="usage">ai-input</permits>
    <payment type="use"><amount currency="USD">0.015</amount></payment>
  </license></content></rsl>"#;

/// Catches a refusal on the page's own turn that drops what observe was
/// carrying: the host's breach, the unmet payment term and the allowance
/// breach are each kept once, the reservation is released naming the delay,
/// and nothing is sent. The robots file states no preference: one that
/// disallows AI input is ruled on in place of the payment term.
#[cfg(unix)]
#[test]
fn a_refused_page_turn_keeps_host_declaration_and_allowance_breaches() {
    let site = licensed_origin(LICENSED_THIRTY_SECONDS, PRICED_LICENCE);
    let home = tempfile::tempdir().expect("tempdir");
    // SAFETY: `geteuid` has no arguments or memory preconditions.
    let uid = unsafe { libc::geteuid() };
    std::fs::write(
        home.path().join("policy.json"),
        json!({
            "policy_mode": "observe",
            "allow_private_hosts": true,
            "constraints": [{"kind": "allowed_source_host", "host": "www.gov.uk"}],
            "principals": [{
                "principal": "capped", "os_user": uid,
                "allowances": [{
                    "period": "day",
                    "amount": {"currency": "USD", "micros": 5_000},
                    "timezone": "UTC",
                }],
            }],
        })
        .to_string(),
    )
    .expect("policy");
    cache_declarations(
        home.path(),
        &site,
        LICENSED_THIRTY_SECONDS,
        Some(PRICED_LICENCE),
    );
    // The licence is current, so the page alone takes a turn: 35 seconds to
    // the seeded turn and 30 after it is 65, over the 60-second budget.
    seed_turn(
        home.path(),
        "127.0.0.1",
        chrono::Utc::now() + chrono::Duration::seconds(35),
    );

    let responses = converse(home.path(), &[fetch(1, &site.url("/article"))]);
    assert_eq!(responses[0]["result"]["isError"], true, "{}", responses[0]);
    assert!(
        site.requests().is_empty(),
        "the origin was asked: {:?}",
        site.requests()
    );
    let recorded = crossings(home.path());
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0]["event"], "crossing_refused");
    let payload = &recorded[0]["payload"];
    assert_eq!(
        payload["declarations"]["robots"]["delay"]["outcome"],
        "refused"
    );
    assert_eq!(payload["declarations"]["licences"][0]["cache"], "reused");
    let breach = payload["breach"].as_str().expect("the breaches are kept");
    for sentence in [
        HOST_CARRIED,
        "payment term is unmet",
        "cumulative allowance",
    ] {
        assert_eq!(breach.matches(sentence).count(), 1, "{sentence}: {breach}");
    }
    let allowance = &payload["allowance"];
    assert_eq!(
        allowance["decision"], "proceeded_with_breach",
        "{allowance}"
    );
    assert!(
        allowance
            .to_string()
            .contains("Crawl-delay refused the fetch before the request"),
        "{allowance}"
    );
    assert!(
        allowance.to_string().contains("released"),
        "the reservation is released: {allowance}"
    );
}
