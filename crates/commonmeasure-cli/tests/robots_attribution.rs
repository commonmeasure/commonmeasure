//! Robots attribution and redirect scope, driven through the real binary
//! over loopback origins: which URL a `robots.txt` rule was applied to,
//! which file, group and rule decided it, what the policy mode did, and
//! which requests were actually made.
//!
//! Two origins stand in for a link shortener and the page it points at. They
//! are both loopback, on different host names so each has its own
//! `robots.txt` and its own cache entry, and the policy opts into private
//! addresses to reach them. Nothing else is substituted.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use commonmeasure_http::{Response, Server, ServerHandle};
use serde_json::{Value, json};

const WILDCARD_DISALLOW: &str = "User-agent: *\nDisallow: /\n";
const WILDCARD_PATTERN: &str = "User-agent: *\nDisallow: /private*\nAllow: /\n";
const SPECIFIC_ALLOW: &str = "User-agent: *\nDisallow: /\nAllow: /public/\n";
const BY_NAME: &str = "\
User-agent: *
Disallow: /

User-agent: CommonMeasureBot
Allow: /
Disallow: /members/
";
const ALLOW_ALL: &str = "User-agent: *\nAllow: /\n";
/// How every `Disallow` refusal ends, in every mode.
const REFUSED: &str = "A `Disallow` binds in every policy mode: refused before the request.";

/// One loopback origin: its `robots.txt`, the request targets it has seen in
/// order, and where a `/s/…` path redirects when it is a shortener.
struct Origin {
    handle: ServerHandle,
    host: &'static str,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Origin {
    fn url(&self, path: &str) -> String {
        format!("http://{}:{}{path}", self.host, self.handle.addr().port())
    }

    fn robots_url(&self) -> String {
        self.url("/robots.txt")
    }

    /// Every page and `robots.txt` request this origin answered, in order.
    /// The Content Telemetry manifest probe that follows a delivered page
    /// is discovery, not a rule check, and is left out so the sequence
    /// asserted is the one the rules governed.
    fn requests(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|target| target.as_str() != "/.well-known/content-telemetry.json")
            .cloned()
            .collect()
    }
}

/// `host` is the name the tests address the origin by; the socket is
/// loopback either way. A `/s/…` target answers `302` to `redirect_to` when
/// one is given; any other target answers text.
fn origin(host: &'static str, robots: &'static str, redirect_to: Option<String>) -> Origin {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |request| {
            log.lock().unwrap().push(request.target.clone());
            if request.target == "/robots.txt" {
                return Response::text(200, robots);
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

/// A shortener on `127.0.0.1` whose `/s/…` paths redirect to `/landing` on a
/// destination addressed as `localhost`, each with its own `robots.txt`.
fn shortener_and_destination(
    shortener_robots: &'static str,
    destination_robots: &'static str,
) -> (Origin, Origin) {
    let destination = origin("localhost", destination_robots, None);
    let landing = destination.url("/landing");
    let shortener = origin("127.0.0.1", shortener_robots, Some(landing));
    (shortener, destination)
}

fn converse(home: &Path, requests: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["mcp", "--host", "claude-code", "--session", "test-session"])
        .env("COMMONMEASURE_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the binary should start");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        for request in requests {
            writeln!(stdin, "{request}").expect("write request");
        }
    }
    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success(), "the server should exit cleanly");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
        .collect()
}

fn fetch(url: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "context_fetch", "arguments": {"url": url}},
    })
}

fn payload(response: &Value) -> Value {
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .expect("a tool result carries text");
    serde_json::from_str(text).expect("the payload is JSON")
}

fn error_text(response: &Value) -> String {
    response["result"]["content"][0]["text"]
        .as_str()
        .expect("an error result carries text")
        .to_owned()
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

fn home_with_mode(mode: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        home.path().join("policy.json"),
        format!(r#"{{"policy_mode":"{mode}","allow_private_hosts":true}}"#),
    )
    .expect("policy");
    home
}

/// The `robots` attribution on a crossing record, checked field by field
/// against the URL it should be about and the rule that should have decided.
struct Expected<'a> {
    requested_url: &'a str,
    robots_url: &'a str,
    group: &'a str,
    group_is_wildcard: bool,
    access_rule: &'a str,
    access_rule_wildcard: bool,
    mode: &'a str,
    outcome: &'a str,
}

fn assert_attribution(recorded: &Value, expected: &Expected<'_>) {
    assert_eq!(
        recorded["requested_url"], expected.requested_url,
        "{recorded}"
    );
    assert_eq!(recorded["url"], expected.robots_url, "{recorded}");
    let reading = &recorded["reading"];
    assert_eq!(reading["group"], expected.group, "{recorded}");
    assert_eq!(
        reading["group_is_wildcard"], expected.group_is_wildcard,
        "{recorded}"
    );
    assert_eq!(reading["access_rule"], expected.access_rule, "{recorded}");
    assert_eq!(
        reading["access_rule_wildcard"], expected.access_rule_wildcard,
        "{recorded}"
    );
    assert_eq!(recorded["mode"], expected.mode, "{recorded}");
    assert_eq!(recorded["outcome"], expected.outcome, "{recorded}");
}

/// The same attribution as the agent is shown it in a tool result.
fn assert_explained(shown: &Value, expected: &Expected<'_>) {
    assert_eq!(shown["requested_url"], expected.requested_url, "{shown}");
    assert_eq!(shown["robots_url"], expected.robots_url, "{shown}");
    assert_eq!(shown["group"], expected.group, "{shown}");
    assert_eq!(
        shown["group_is_wildcard"], expected.group_is_wildcard,
        "{shown}"
    );
    assert_eq!(shown["access_rule"], expected.access_rule, "{shown}");
    assert_eq!(
        shown["access_rule_wildcard"], expected.access_rule_wildcard,
        "{shown}"
    );
    assert_eq!(shown["mode"], expected.mode, "{shown}");
    assert_eq!(shown["outcome"], expected.outcome, "{shown}");
    let explanation = shown["explanation"].as_str().expect("an explanation");
    assert!(
        explanation.contains(expected.requested_url)
            && explanation.contains(expected.robots_url)
            && explanation.contains(expected.access_rule),
        "{explanation}"
    );
}

#[test]
fn a_wildcard_group_disallow_is_named_as_such_and_strict_stops_before_the_request() {
    let site = origin("127.0.0.1", WILDCARD_DISALLOW, None);
    let home = home_with_mode("strict");
    let article = site.url("/article");

    let responses = converse(home.path(), &[fetch(&article)]);
    assert_eq!(responses[0]["result"]["isError"], true, "{}", responses[0]);
    let detail = error_text(&responses[0]);
    for needed in [
        "refused before the crossing",
        article.as_str(),
        site.robots_url().as_str(),
        "`User-agent: *` group",
        "addresses every fetcher because no group names CommonMeasureBot",
        "`Disallow: /`, a literal path prefix",
        REFUSED,
    ] {
        assert!(detail.contains(needed), "missing {needed:?} in {detail}");
    }
    assert_eq!(
        site.requests(),
        vec!["/robots.txt"],
        "the disallowed page was never requested"
    );

    let recorded = crossings(home.path());
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0]["event"], "crossing_refused");
    let payload = &recorded[0]["payload"];
    assert_eq!(payload["url"], article);
    assert!(payload["http_status"].is_null(), "nothing was requested");
    assert_attribution(
        &payload["declarations"]["robots"],
        &Expected {
            requested_url: &article,
            robots_url: &site.robots_url(),
            group: "*",
            group_is_wildcard: true,
            access_rule: "Disallow: /",
            access_rule_wildcard: false,
            mode: "strict",
            outcome: "refused",
        },
    );
    assert_eq!(
        payload["declarations"]["robots"]["reading"]["crawlable"],
        false
    );
    assert!(
        payload["declarations"]["redirects"].is_null(),
        "nothing redirected"
    );
}

/// Until WP-29 observe carried this `Disallow` with the breach recorded; a
/// `Disallow` now binds in every mode, so observe refuses it before the
/// request, with the wildcard pattern still named as one.
#[test]
fn a_wildcard_pattern_rule_is_identified_as_a_wildcard_and_observe_refuses_it() {
    let site = origin("127.0.0.1", WILDCARD_PATTERN, None);
    let home = home_with_mode("observe");
    let report = site.url("/private/report");

    let responses = converse(home.path(), &[fetch(&report)]);
    assert_eq!(responses[0]["result"]["isError"], true, "{}", responses[0]);
    let detail = error_text(&responses[0]);
    for needed in ["`Disallow: /private*`, a wildcard pattern", REFUSED] {
        assert!(detail.contains(needed), "missing {needed:?} in {detail}");
    }
    assert_eq!(
        site.requests(),
        vec!["/robots.txt"],
        "the disallowed page was never requested"
    );

    let recorded = crossings(home.path());
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0]["event"], "crossing_refused");
    assert!(recorded[0]["payload"]["breach"].is_null());
    assert_attribution(
        &recorded[0]["payload"]["declarations"]["robots"],
        &Expected {
            requested_url: &report,
            robots_url: &site.robots_url(),
            group: "*",
            group_is_wildcard: true,
            access_rule: "Disallow: /private*",
            access_rule_wildcard: true,
            mode: "observe",
            outcome: "refused",
        },
    );
}

#[test]
fn a_more_specific_allow_beats_the_wildcard_disallow_and_the_record_names_it() {
    let site = origin("127.0.0.1", SPECIFIC_ALLOW, None);
    let home = home_with_mode("strict");
    let public = site.url("/public/page");
    let secret = site.url("/secret");

    let responses = converse(home.path(), &[fetch(&public), fetch(&secret)]);
    assert_eq!(responses[0]["result"]["isError"], false, "{}", responses[0]);
    let result = payload(&responses[0]);
    let allowed = Expected {
        requested_url: &public,
        robots_url: &site.robots_url(),
        group: "*",
        group_is_wildcard: true,
        access_rule: "Allow: /public/",
        access_rule_wildcard: false,
        mode: "strict",
        outcome: "allowed",
    };
    assert_explained(&result["declarations"]["robots"], &allowed);
    assert!(result["breach"].is_null(), "{result}");

    assert_eq!(responses[1]["result"]["isError"], true, "{}", responses[1]);
    assert!(
        error_text(&responses[1]).contains("`Disallow: /`"),
        "{}",
        error_text(&responses[1])
    );
    assert_eq!(
        site.requests(),
        vec!["/robots.txt", "/public/page"],
        "robots.txt is read once and the disallowed page is never requested"
    );

    let recorded = crossings(home.path());
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[0]["event"], "crossing_mediated");
    assert_attribution(&recorded[0]["payload"]["declarations"]["robots"], &allowed);
    assert_eq!(recorded[1]["event"], "crossing_refused");
    assert_attribution(
        &recorded[1]["payload"]["declarations"]["robots"],
        &Expected {
            requested_url: &secret,
            robots_url: &site.robots_url(),
            group: "*",
            group_is_wildcard: true,
            access_rule: "Disallow: /",
            access_rule_wildcard: false,
            mode: "strict",
            outcome: "refused",
        },
    );
    assert_eq!(
        recorded[1]["payload"]["declarations"]["robots"]["cache"],
        "reused"
    );
}

#[test]
fn the_commonmeasurebot_group_governs_where_the_file_names_it() {
    let site = origin("127.0.0.1", BY_NAME, None);
    let home = home_with_mode("strict");
    let news = site.url("/news");
    let members = site.url("/members/only");

    let responses = converse(home.path(), &[fetch(&news), fetch(&members)]);
    assert_eq!(responses[0]["result"]["isError"], false, "{}", responses[0]);
    let result = payload(&responses[0]);
    let named = Expected {
        requested_url: &news,
        robots_url: &site.robots_url(),
        group: "CommonMeasureBot",
        group_is_wildcard: false,
        access_rule: "Allow: /",
        access_rule_wildcard: false,
        mode: "strict",
        outcome: "allowed",
    };
    assert_explained(&result["declarations"]["robots"], &named);
    assert!(
        result["declarations"]["robots"]["explanation"]
            .as_str()
            .unwrap()
            .contains("`User-agent: CommonMeasureBot` group, which names CommonMeasureBot"),
        "{result}"
    );

    assert_eq!(responses[1]["result"]["isError"], true, "{}", responses[1]);
    let detail = error_text(&responses[1]);
    assert!(
        detail.contains("`User-agent: CommonMeasureBot` group")
            && detail.contains("`Disallow: /members/`, a literal path prefix")
            && !detail.contains("`User-agent: *`"),
        "the by-name group is attributed, not the wildcard group it overrides: {detail}"
    );
    assert_eq!(site.requests(), vec!["/robots.txt", "/news"]);

    let recorded = crossings(home.path());
    assert_eq!(recorded.len(), 2);
    assert_attribution(&recorded[0]["payload"]["declarations"]["robots"], &named);
    assert_attribution(
        &recorded[1]["payload"]["declarations"]["robots"],
        &Expected {
            requested_url: &members,
            robots_url: &site.robots_url(),
            group: "CommonMeasureBot",
            group_is_wildcard: false,
            access_rule: "Disallow: /members/",
            access_rule_wildcard: false,
            mode: "strict",
            outcome: "refused",
        },
    );
}

/// Until WP-29 observe carried the shortener's `Disallow` and followed the
/// redirect. Observe now refuses the shortener before asking for it, the
/// rule is attributed to the shortener, and the destination is never
/// reached.
#[test]
fn a_shorteners_disallow_is_attributed_to_the_shortener_and_observe_refuses_the_hop() {
    let (shortener, destination) = shortener_and_destination(WILDCARD_DISALLOW, ALLOW_ALL);
    let home = home_with_mode("observe");
    let short = shortener.url("/s/abc");

    let responses = converse(home.path(), &[fetch(&short)]);
    assert_eq!(responses[0]["result"]["isError"], true, "{}", responses[0]);
    let detail = error_text(&responses[0]);
    assert!(
        detail.contains(&format!(
            "{} disallows CommonMeasureBot at {short}",
            shortener.robots_url()
        )) && detail.contains(REFUSED),
        "{detail}"
    );
    assert_eq!(shortener.requests(), vec!["/robots.txt"]);
    assert!(
        destination.requests().is_empty(),
        "a redirect that was never followed reaches nothing: {:?}",
        destination.requests()
    );

    let recorded = crossings(home.path());
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0]["event"], "crossing_refused");
    let payload = &recorded[0]["payload"];
    assert_eq!(payload["url"], short);
    assert!(payload["breach"].is_null(), "{payload}");
    assert_attribution(
        &payload["declarations"]["robots"],
        &Expected {
            requested_url: &short,
            robots_url: &shortener.robots_url(),
            group: "*",
            group_is_wildcard: true,
            access_rule: "Disallow: /",
            access_rule_wildcard: false,
            mode: "observe",
            outcome: "refused",
        },
    );
    assert!(payload["declarations"]["redirects"].is_null());
}

#[test]
fn strict_refuses_a_disallowed_shortener_before_requesting_it_and_never_reaches_the_destination() {
    let (shortener, destination) = shortener_and_destination(WILDCARD_DISALLOW, ALLOW_ALL);
    let home = home_with_mode("strict");
    let short = shortener.url("/s/abc");

    let responses = converse(home.path(), &[fetch(&short)]);
    assert_eq!(responses[0]["result"]["isError"], true, "{}", responses[0]);
    let detail = error_text(&responses[0]);
    assert!(
        detail.contains(&format!(
            "{} disallows CommonMeasureBot at {short}",
            shortener.robots_url()
        )),
        "{detail}"
    );
    assert_eq!(shortener.requests(), vec!["/robots.txt"]);
    assert!(
        destination.requests().is_empty(),
        "a redirect that was never followed reaches nothing: {:?}",
        destination.requests()
    );

    let recorded = crossings(home.path());
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0]["event"], "crossing_refused");
    let payload = &recorded[0]["payload"];
    assert_eq!(payload["url"], short);
    assert_attribution(
        &payload["declarations"]["robots"],
        &Expected {
            requested_url: &short,
            robots_url: &shortener.robots_url(),
            group: "*",
            group_is_wildcard: true,
            access_rule: "Disallow: /",
            access_rule_wildcard: false,
            mode: "strict",
            outcome: "refused",
        },
    );
    assert!(payload["declarations"]["redirects"].is_null());
}

#[test]
fn a_disallowed_destination_is_refused_before_the_hop_is_requested() {
    let (shortener, destination) = shortener_and_destination(ALLOW_ALL, WILDCARD_DISALLOW);
    let home = home_with_mode("strict");
    let short = shortener.url("/s/abc");
    let landing = destination.url("/landing");

    let responses = converse(home.path(), &[fetch(&short)]);
    assert_eq!(responses[0]["result"]["isError"], true, "{}", responses[0]);
    let detail = error_text(&responses[0]);
    for needed in [
        format!("a redirect to {landing}"),
        format!(
            "{} disallows CommonMeasureBot at {landing}",
            destination.robots_url()
        ),
        REFUSED.to_owned(),
    ] {
        assert!(detail.contains(&needed), "missing {needed:?} in {detail}");
    }
    assert_eq!(
        shortener.requests(),
        vec!["/robots.txt", "/s/abc"],
        "the allowed shortener was asked and answered with the redirect"
    );
    assert_eq!(
        destination.requests(),
        vec!["/robots.txt"],
        "the destination's rules were read and the page was never requested"
    );

    let recorded = crossings(home.path());
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0]["event"], "crossing_refused");
    let payload = &recorded[0]["payload"];
    assert_eq!(
        payload["url"], landing,
        "the refused hop is the URL on record"
    );
    assert!(payload["http_status"].is_null());
    assert_attribution(
        &payload["declarations"]["robots"],
        &Expected {
            requested_url: &landing,
            robots_url: &destination.robots_url(),
            group: "*",
            group_is_wildcard: true,
            access_rule: "Disallow: /",
            access_rule_wildcard: false,
            mode: "strict",
            outcome: "refused",
        },
    );
    let hops = payload["declarations"]["redirects"]
        .as_array()
        .expect("the shortener's own evaluation is kept");
    assert_eq!(hops.len(), 1);
    assert_attribution(
        &hops[0],
        &Expected {
            requested_url: &short,
            robots_url: &shortener.robots_url(),
            group: "*",
            group_is_wildcard: true,
            access_rule: "Allow: /",
            access_rule_wildcard: false,
            mode: "strict",
            outcome: "allowed",
        },
    );
}

/// Until WP-29 observe and prefer followed a redirect to a disallowed
/// destination and carried the breach. Each hop is judged by its own host's
/// `robots.txt`, and a disallowed hop now refuses the crossing in every
/// mode before that hop is asked for.
#[test]
fn observe_and_prefer_refuse_a_disallowed_destination_hop_with_the_destination_named() {
    for mode in ["observe", "prefer"] {
        let (shortener, destination) = shortener_and_destination(ALLOW_ALL, WILDCARD_DISALLOW);
        let home = home_with_mode(mode);
        let short = shortener.url("/s/abc");
        let landing = destination.url("/landing");

        let responses = converse(home.path(), &[fetch(&short)]);
        assert_eq!(
            responses[0]["result"]["isError"], true,
            "{mode}: {}",
            responses[0]
        );
        let detail = error_text(&responses[0]);
        for needed in [
            format!("a redirect to {landing}"),
            format!(
                "{} disallows CommonMeasureBot at {landing}",
                destination.robots_url()
            ),
            REFUSED.to_owned(),
        ] {
            assert!(
                detail.contains(&needed),
                "{mode}: missing {needed:?} in {detail}"
            );
        }
        assert!(
            !detail.contains(&format!("at {short}")),
            "{mode}: the shortener allowed the hop and is not blamed: {detail}"
        );
        assert_eq!(
            shortener.requests(),
            vec!["/robots.txt", "/s/abc"],
            "{mode}"
        );
        assert_eq!(
            destination.requests(),
            vec!["/robots.txt"],
            "{mode}: the destination's rules were read and the page was never requested"
        );

        let recorded = crossings(home.path());
        assert_eq!(recorded.len(), 1, "{mode}");
        assert_eq!(recorded[0]["event"], "crossing_refused", "{mode}");
        let payload = &recorded[0]["payload"];
        assert_eq!(payload["url"], landing, "{mode}");
        assert!(payload["http_status"].is_null(), "{mode}");
        assert!(payload["breach"].is_null(), "{mode}: {payload}");
        assert_attribution(
            &payload["declarations"]["robots"],
            &Expected {
                requested_url: &landing,
                robots_url: &destination.robots_url(),
                group: "*",
                group_is_wildcard: true,
                access_rule: "Disallow: /",
                access_rule_wildcard: false,
                mode,
                outcome: "refused",
            },
        );
        let hops = payload["declarations"]["redirects"]
            .as_array()
            .expect("hops");
        assert_eq!(hops[0]["requested_url"], short, "{mode}");
        assert_eq!(hops[0]["outcome"], "allowed", "{mode}");
        assert_eq!(hops[0]["mode"], mode, "{mode}");
    }
}

#[test]
fn an_independently_obtained_destination_url_receives_its_own_check() {
    // The shortener disallows everything and the destination allows it:
    // named directly, the destination is judged by its own file and the
    // shortener is never consulted.
    let (shortener, destination) = shortener_and_destination(WILDCARD_DISALLOW, ALLOW_ALL);
    let home = home_with_mode("strict");
    let landing = destination.url("/landing");
    let responses = converse(home.path(), &[fetch(&landing)]);
    assert_eq!(responses[0]["result"]["isError"], false, "{}", responses[0]);
    let result = payload(&responses[0]);
    assert_explained(
        &result["declarations"]["robots"],
        &Expected {
            requested_url: &landing,
            robots_url: &destination.robots_url(),
            group: "*",
            group_is_wildcard: true,
            access_rule: "Allow: /",
            access_rule_wildcard: false,
            mode: "strict",
            outcome: "allowed",
        },
    );
    assert!(
        result["declarations"]["redirects"]
            .as_array()
            .is_some_and(Vec::is_empty)
    );
    assert!(
        shortener.requests().is_empty(),
        "{:?}",
        shortener.requests()
    );
    assert_eq!(destination.requests(), vec!["/robots.txt", "/landing"]);

    // The reverse: the destination disallows, and naming it directly earns
    // the same refusal a redirect to it would, with no redirect to blame.
    let (shortener, destination) = shortener_and_destination(ALLOW_ALL, WILDCARD_DISALLOW);
    let home = home_with_mode("strict");
    let landing = destination.url("/landing");
    let responses = converse(home.path(), &[fetch(&landing)]);
    assert_eq!(responses[0]["result"]["isError"], true, "{}", responses[0]);
    let detail = error_text(&responses[0]);
    assert!(
        detail.contains(&format!(
            "{} disallows CommonMeasureBot at {landing}",
            destination.robots_url()
        )) && !detail.contains("a redirect to"),
        "{detail}"
    );
    assert!(shortener.requests().is_empty());
    assert_eq!(destination.requests(), vec!["/robots.txt"]);
    let recorded = crossings(home.path());
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0]["event"], "crossing_refused");
    assert_eq!(recorded[0]["payload"]["url"], landing);
    assert_eq!(
        recorded[0]["payload"]["declarations"]["robots"]["requested_url"],
        landing
    );
    assert!(recorded[0]["payload"]["declarations"]["redirects"].is_null());
}
