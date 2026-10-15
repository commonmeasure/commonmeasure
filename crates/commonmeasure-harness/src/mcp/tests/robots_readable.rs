//! An origin's own `robots.txt` is readable whatever the source's terms say
//! about AI input (owner decision, 13 October 2026): the file holds the
//! rules, and an agent that cannot read it cannot report or keep them.
//!
//! These cases fetch `/robots.txt` from a publisher whose terms refuse its
//! content pages (an RSL licence that permits no AI input, one that permits
//! it only under a payment, a `Content-Signal: ai-input=no` line, a
//! reporting demand this session cannot meet, a licence that cannot be
//! read) and show the file delivered while `/article` is still refused. The
//! exemption covers only the URL `robots_url_of` maps to itself: a query, a
//! case variant of the path and a redirect to a content page are ruled on
//! the terms, and the operator's host policy still binds.
//!
//! Every case runs through `McpServer::tool_fetch` against a loopback
//! publisher, in `observe`, `prefer` and `strict` unless it names a mode.
//! It is a child of `mcp.rs`'s test module and uses its helpers (`server`,
//! `bind_local`, `crossings`).

use super::*;

const MODES: [&str; 3] = ["observe", "prefer", "strict"];

/// A `robots.txt` that names the licence and allows every path.
const ROBOTS: &str = "License: /license.xml\nUser-agent: *\nAllow: /\n";

/// A licence that permits search and nothing else, so no licence permits
/// AI input.
const SEARCH_ONLY: &[u8] = br#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/"><license><permits type="usage">search</permits></license></content></rsl>"#;

/// A licence that permits AI input under a subscription, which no rail on
/// this edge can settle.
const SUBSCRIPTION: &[u8] = br#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/"><license><permits type="usage">ai-input</permits><payment type="subscription"/></license></content></rsl>"#;

/// A free licence whose telemetry reporting demand this session cannot
/// meet: it is enrolled with no hub.
const REPORTING: &[u8] = br#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/"><license><permits type="usage">ai-input</permits><payment type="free"/><reporting type="telemetry" profile="https://contenttelemetry.org/profiles/spur" endpoint="https://telemetry.example.com/v1/events"><![CDATA[{"conformance_level":"grounding","privacy_level":"minimal"}]]></reporting></license></content></rsl>"#;

/// What the publisher serves at `/license.xml`: a status and a body.
type Licence = (u16, &'static [u8]);

/// A publisher whose `robots.txt` is `robots`, whose `/license.xml` is
/// `licence` and whose every other path but the well-known probes is a
/// content page. Counts the requests for content pages.
fn publisher(
    robots: &'static str,
    licence: Licence,
) -> (
    commonmeasure_http::ServerHandle,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
    let pages = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = std::sync::Arc::clone(&pages);
    let site = bind_local()
        .spawn(move |request| {
            let target = request.target.as_str();
            if target == "/robots.txt" {
                return Response::text(200, robots);
            }
            if target == "/license.xml" {
                return Response::new(licence.0, licence.1.to_vec());
            }
            if target.starts_with("/.well-known/") {
                return Response::text(404, "none");
            }
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Response::text(200, "the article")
        })
        .expect("spawn");
    (site, pages)
}

/// One fetch of `path` at a fresh `publisher(robots, licence)` under
/// `policy`: the result, the one crossing recorded and the content pages
/// asked for.
fn fetch(
    path: &str,
    robots: &'static str,
    licence: Licence,
    policy: &Value,
) -> (Result<String, String>, Value, usize) {
    let (mut site, pages) = publisher(robots, licence);
    let (home, mut server) = server(&policy.to_string());
    let result = server
        .tool_fetch(&json!({"url": format!("{}{path}", site.url())}))
        .map(|delivered| delivered.to_string());
    let recorded = crossings(home.path());
    assert_eq!(recorded.len(), 1, "{path}: {recorded:?}");
    let pages = pages.load(std::sync::atomic::Ordering::SeqCst);
    site.stop();
    (result, recorded[0].clone(), pages)
}

fn in_mode(mode: &str) -> Value {
    json!({"policy_mode": mode, "allow_private_hosts": true})
}

/// In every mode `/robots.txt` is delivered with nothing refused, and
/// `/article` is refused before it is asked for, the agent told `told`.
fn robots_delivered_and_page_refused(robots: &'static str, licence: Licence, told: &str) {
    for mode in MODES {
        let (result, crossing, pages) = fetch("/robots.txt", robots, licence, &in_mode(mode));
        let delivered = result.unwrap_or_else(|error| panic!("{mode}: {error}"));
        assert!(delivered.contains("User-agent: *"), "{mode}: {delivered}");
        assert_eq!(pages, 0, "{mode}");
        assert_eq!(crossing["event"], "crossing_mediated", "{mode}: {crossing}");
        assert!(
            crossing["payload"]
                .get("refusal")
                .is_none_or(Value::is_null),
            "{mode}: {crossing}"
        );

        refused(PAGE, robots, licence, mode, told);
    }
}

/// The page the refused cases ask for.
const PAGE: &str = "/article";

/// A fetch of `path` in `mode` is refused before the page is asked for,
/// and the agent is told `told`.
fn refused(path: &str, robots: &'static str, licence: Licence, mode: &str, told: &str) {
    let (result, crossing, pages) = fetch(path, robots, licence, &in_mode(mode));
    let error = result.expect_err(mode);
    assert!(
        error.starts_with("refused before the crossing: "),
        "{mode} {path}: {error}"
    );
    assert!(error.contains(told), "{mode} {path}: {error}");
    assert_eq!(pages, 0, "{mode} {path}: the page was never asked for");
    assert_eq!(crossing["event"], "crossing_refused", "{mode} {path}");
}

#[test]
fn a_licence_permitting_no_ai_input_delivers_robots_and_refuses_a_page() {
    robots_delivered_and_page_refused(
        ROBOTS,
        (200, SEARCH_ONLY),
        "The source disallows AI input (in its RSL licence).",
    );
}

#[test]
fn a_licence_permitting_ai_input_under_payment_delivers_robots_and_refuses_a_page() {
    robots_delivered_and_page_refused(
        ROBOTS,
        (200, SUBSCRIPTION),
        "payment type subscription, and this edge holds no settlement rail",
    );
}

#[test]
fn a_content_signal_refusing_ai_input_delivers_robots_and_refuses_a_page() {
    robots_delivered_and_page_refused(
        "User-agent: *\nAllow: /\nContent-Signal: ai-input=no\n",
        (404, b"none"),
        "The source disallows AI input (in a Content-Signal line in its robots.txt).",
    );
}

#[test]
fn a_reporting_demand_this_session_cannot_meet_delivers_robots_and_refuses_a_page() {
    robots_delivered_and_page_refused(ROBOTS, (200, REPORTING), "requires telemetry reporting");
}

#[test]
fn an_unread_licence_delivers_robots_and_refuses_a_page() {
    robots_delivered_and_page_refused(ROBOTS, (500, b"broken"), "could not be read");
}

/// `robots_url_of` drops a query and writes the path in lower case, so
/// neither of these is the origin's `robots.txt` and the terms rule on
/// them as on any page.
#[test]
fn a_query_or_a_case_variant_of_robots_is_ruled_on_the_terms() {
    for path in ["/robots.txt?x=1", "/ROBOTS.TXT", "/a/robots.txt"] {
        for mode in MODES {
            refused(
                path,
                ROBOTS,
                (200, SEARCH_ONLY),
                mode,
                "The source disallows AI input (in its RSL licence).",
            );
        }
    }
}

/// A `robots.txt` that redirects to a content page is ruled on the terms
/// at that hop: the licence the page names in a `Link` header permits no
/// AI input, so the crossing is refused. The edge's own probe of the file
/// follows the redirect too, so the page's requests are not counted.
#[test]
fn a_robots_redirecting_to_a_content_page_is_ruled_on_the_terms() {
    for mode in MODES {
        let mut site = bind_local()
            .spawn(|request| match request.target.as_str() {
                "/robots.txt" => {
                    let mut response = Response::new(302, Vec::new());
                    response.headers.set("Location", "/article");
                    response
                }
                "/license.xml" => Response::new(200, SEARCH_ONLY.to_vec()),
                target if target.starts_with("/.well-known/") => Response::text(404, "none"),
                _ => {
                    let mut response = Response::text(200, "the article");
                    response.headers.set(
                        "Link",
                        r#"</license.xml>; rel="license"; type="application/rsl+xml""#,
                    );
                    response
                }
            })
            .expect("spawn");
        let (home, mut server) = server(&in_mode(mode).to_string());
        let result = server.tool_fetch(&json!({"url": format!("{}/robots.txt", site.url())}));
        let recorded = crossings(home.path());
        site.stop();
        let error = result.expect_err(mode).to_string();
        assert!(
            error.contains("The source disallows AI input (in its RSL licence)."),
            "{mode}: {error}"
        );
        assert!(!error.contains("the article"), "{mode}: {error}");
        assert_eq!(recorded.len(), 1, "{mode}: {recorded:?}");
        assert_eq!(recorded[0]["event"], "crossing_refused", "{mode}");
    }
}

/// The operator's host policy binds the file as it binds any page: `strict`
/// refuses a denied host's `robots.txt` before the request, and `observe`
/// delivers it with the breach on the record.
#[test]
fn a_host_the_operator_denies_is_ruled_on_for_its_robots() {
    let denied = |mode: &str| {
        json!({
            "policy_mode": mode,
            "allow_private_hosts": true,
            "constraints": [{"kind": "denied_source_host", "host": "127.0.0.1"}],
        })
    };
    let (result, crossing, _) = fetch("/robots.txt", ROBOTS, (200, SEARCH_ONLY), &denied("strict"));
    let error = result.expect_err("strict");
    assert!(
        error.starts_with("refused before the crossing: "),
        "{error}"
    );
    assert!(!error.contains("AI input"), "{error}");
    assert_eq!(crossing["event"], "crossing_refused", "{crossing}");

    let (result, crossing, _) = fetch(
        "/robots.txt",
        ROBOTS,
        (200, SEARCH_ONLY),
        &denied("observe"),
    );
    result.unwrap_or_else(|error| panic!("observe: {error}"));
    assert_eq!(crossing["event"], "crossing_mediated", "{crossing}");
    assert!(
        crossing["payload"]["breach"]
            .as_str()
            .is_some_and(|breach| !breach.is_empty() && !breach.contains("AI input")),
        "{crossing}"
    );
}

/// The licence's quoted price is not reserved against the principal's
/// allowance for the file, since the licence does not govern it: a ledger
/// the gate cannot read refuses `/article` in `strict` and leaves
/// `/robots.txt` delivered with no allowance on its record. Unix only: the
/// allowance binds to the effective uid.
#[cfg(unix)]
#[test]
fn the_allowance_is_not_consulted_for_robots() {
    const PRICED: &[u8] = br#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/"><license><permits type="usage">ai-input</permits><payment type="use"><amount currency="USD">0.015</amount></payment></license></content></rsl>"#;
    let uid = crate::policy::trusted_os_user().expect("an effective uid");
    let policy = json!({
        "policy_mode": "strict",
        "allow_private_hosts": true,
        "principals": [{
            "principal": "capped", "os_user": uid,
            "allowances": [{
                "period": "day",
                "amount": {"currency": "USD", "micros": 1_000_000},
                "timezone": "UTC",
            }],
        }],
    })
    .to_string();
    for (path, delivered) in [("/robots.txt", true), (PAGE, false)] {
        let (mut site, _) = publisher(ROBOTS, (200, PRICED));
        let (home, mut server) = server(&policy);
        let ledger = home.path().join("allowance/ledger.ndjson");
        std::fs::create_dir_all(ledger.parent().expect("parent")).expect("directory");
        std::fs::write(&ledger, "invalid ledger\n").expect("ledger");
        let result = server.tool_fetch(&json!({"url": format!("{}{path}", site.url())}));
        let crossing = crossings(home.path()).pop().expect("a crossing");
        site.stop();
        if delivered {
            result.unwrap_or_else(|error| panic!("{path}: {error}"));
            assert_eq!(crossing["event"], "crossing_mediated", "{crossing}");
            assert!(
                crossing["payload"]
                    .get("allowance")
                    .is_none_or(Value::is_null),
                "{crossing}"
            );
        } else {
            let error = result.expect_err(path);
            assert!(error.contains("ledger could not be consulted"), "{error}");
        }
    }
}
