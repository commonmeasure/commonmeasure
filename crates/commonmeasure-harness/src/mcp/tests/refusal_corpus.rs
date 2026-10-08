//! A refused crossing gives the agent no text from the source.
//!
//! Each case plants a marker in one channel the source controls (a
//! redirect's host and path, a response header, `robots.txt`, a licence's
//! fields, a search result's URL, title and snippet, a certificate name)
//! and runs one refusal or failure path through `tool_fetch` or the
//! search's delivery. The marker must be absent from what the agent reads,
//! present in the source record, and the record must say the crossing was
//! not grounded. `MARKER` is a substring of every planted value, so one
//! check covers every channel of a case; a header the publisher adds to
//! every answer plants the header channels on every case at once.
//!
//! This file is a child of `mcp.rs`'s test module and uses its helpers
//! (`server`, `raw_site`, `raw_answer`, `crossings`), with its own copies of
//! the two it needs beyond them, so that it can be dropped onto an earlier
//! revision to show that it fails there.

use super::*;

/// The substring every planted value carries.
const MARKER: &str = "zqxvmarker";

/// The redirect target's host, with the marker early enough that a cut to
/// 64 characters keeps it. `.invalid` never resolves in DNS.
const HOP_HOST: &str = "zqxvmarker-redirect.invalid";

/// Response headers the publisher adds to every answer: the channels a
/// page or a redirect controls, each with the marker.
const PLANTED_HEADERS: &str = "Retry-After: zqxvmarker-retry\r\n\
                               Content-Usage: zqxvmarker-usage=y\r\n\
                               Link: </zqxvmarker-link>; rel=\"zqxvmarker-rel\"\r\n";

fn answer(head: &str, body: &[u8]) -> Vec<u8> {
    raw_answer(format!("{head}{PLANTED_HEADERS}").as_bytes(), body)
}

fn page_ok() -> Vec<u8> {
    answer("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n", PAGE)
}

fn robots_allow() -> Vec<u8> {
    answer(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n",
        b"User-agent: *\nAllow: /\n",
    )
}

/// Holds a current `robots.txt` of `body` for `url`'s origin, so a crossing
/// reaches that origin's page request without a probe, which would look the
/// name up in DNS.
fn hold_robots(home: &std::path::Path, url: &str, body: &str) {
    let now = Utc::now();
    let robots: discovery::HostRecord = serde_json::from_value(json!({"robots": {
        "url": discovery::robots_url_of(url),
        "fetched_at": now,
        "expires_at": now + discovery::ROBOTS_CACHE_AGE,
        "status": 200,
        "body": body,
    }}))
    .expect("a host record");
    discovery::DeclarationCache::open(home).save(&discovery::origin_key(url), &robots);
}

/// Looks [`HOP_HOST`] up as `address`, and every other name in DNS.
fn hop_at(address: SocketAddr) -> Resolve {
    Box::new(move |hop| {
        if grounding::host_of(hop) == HOP_HOST {
            Ok(vec![address])
        } else {
            system_resolve(hop)
        }
    })
}

fn address_of(base: &str) -> SocketAddr {
    base.trim_start_matches("http://")
        .parse()
        .expect("a loopback address")
}

/// The URL `/page` at the loopback publisher redirects to.
fn hop_url(base: &str) -> String {
    format!(
        "http://{HOP_HOST}:{}/zqxvmarker-path",
        address_of(base).port()
    )
}

/// A loopback publisher whose `/page` redirects to [`hop_url`] and which
/// serves every other path, `robots.txt` included.
fn redirecting(origin: &str, target: &str) -> Vec<u8> {
    match target {
        "/page" => answer(
            &format!("HTTP/1.1 302 Found\r\nLocation: {}\r\n", hop_url(origin)),
            b"",
        ),
        "/robots.txt" => robots_allow(),
        _ => page_ok(),
    }
}

/// The rule, checked: `told` carries no planted value; the last crossing
/// recorded under `home` carries each of `recorded`, is not grounded, and
/// records `told` itself, so that the check holds from the record alone
/// ([`told_holds_no_source_value_of`]).
fn refused_without_source_text(home: &std::path::Path, told: &str, recorded: &[&str]) {
    assert!(
        !told.to_ascii_lowercase().contains(MARKER),
        "the agent read a planted value: {told}"
    );
    let crossing = crossings(home).pop().expect("a crossing is recorded");
    assert_eq!(
        crossing["payload"]["grounded"], false,
        "a refused crossing is not grounded: {crossing}"
    );
    let record = crossing.to_string();
    for value in recorded {
        assert!(
            record.contains(value),
            "the record keeps {value} whole: {record}"
        );
    }
    assert_eq!(
        crossing["payload"]["told"].as_str(),
        Some(told),
        "the record says what the agent was told, whole: {crossing}"
    );
    told_holds_no_source_value_of(&crossing);
}

/// The source's values a crossing record holds: the URL and host it names,
/// what the origin answered with (`challenge`, `content_type`) and every
/// string under `declarations` (the robots.txt rule, group and file, the
/// redirects, each licence's URL and terms, the statements, the headers
/// rendered). Those are the channels a source controls.
fn source_values_of(crossing: &Value) -> Vec<String> {
    fn leaves(value: &Value, into: &mut Vec<String>) {
        match value {
            Value::String(text) => into.push(text.clone()),
            Value::Array(items) => items.iter().for_each(|item| leaves(item, into)),
            Value::Object(fields) => fields.values().for_each(|field| leaves(field, into)),
            _ => {}
        }
    }
    let payload = &crossing["payload"];
    let mut values = Vec::new();
    for field in ["url", "host_name", "challenge", "content_type"] {
        leaves(&payload[field], &mut values);
    }
    leaves(&payload["declarations"], &mut values);
    values
}

/// EDG-129, checked from the record alone: no value the source set, as the
/// record holds it, occurs in `told`. The record's own sentences (`refusal`,
/// `breach`, `failure`) name those values whole and are not source values;
/// the planted marker tells the source's values apart from this edge's
/// fixed words (`refused`, `observe`), which a sentence may well contain.
fn told_holds_no_source_value_of(crossing: &Value) {
    let told = crossing["payload"]["told"]
        .as_str()
        .expect("a refused or failed crossing records what the agent was told");
    let planted: Vec<String> = source_values_of(crossing)
        .into_iter()
        .filter(|value| value.to_ascii_lowercase().contains(MARKER))
        .collect();
    assert!(
        !planted.is_empty(),
        "the record holds the planted value somewhere the source controls: {crossing}"
    );
    for value in planted {
        assert!(
            !told.contains(&value),
            "the record's `told` holds a source value the record names: {value:?} in {told:?}"
        );
    }
    assert!(
        !told.to_ascii_lowercase().contains(MARKER),
        "the record's `told` holds a planted value: {told}"
    );
}

/// Every case: the mode, the policy (`{base}` stands for the publisher's
/// origin), what the publisher answers, and the values the record must
/// keep.
struct Case {
    name: &'static str,
    mode: &'static str,
    policy: String,
    site: fn(&str, &str) -> Vec<u8>,
    /// What to do to the edge and its home before the fetch.
    arrange: fn(&std::path::Path, &mut McpServer, &str),
    /// Planted values the record must carry, beyond the crossing's URL.
    recorded: &'static [&'static str],
    /// What the refusal the agent reads must say, in this edge's words.
    told: &'static str,
}

fn nothing(_: &std::path::Path, _: &mut McpServer, _: &str) {}

fn resolve_hop_to_publisher(home: &std::path::Path, edge: &mut McpServer, base: &str) {
    hold_robots(home, &hop_url(base), "User-agent: *\nAllow: /\n");
    edge.resolve = hop_at(address_of(base));
}

fn resolve_hop_to_closed_port(home: &std::path::Path, edge: &mut McpServer, base: &str) {
    hold_robots(home, &hop_url(base), "User-agent: *\nAllow: /\n");
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("addr")
    };
    edge.resolve = hop_at(closed);
}

fn resolve_hop_to_private(home: &std::path::Path, edge: &mut McpServer, base: &str) {
    hold_robots(home, &hop_url(base), "User-agent: *\nAllow: /\n");
    edge.resolve = hop_at("10.0.0.1:80".parse().expect("an address"));
}

fn hop_in_back_off(home: &std::path::Path, edge: &mut McpServer, base: &str) {
    resolve_hop_to_publisher(home, edge, base);
    crate::crawl_delay::CrawlDelayStore::open(home)
        .answered(HOP_HOST, 503, Some("1800"), Utc::now())
        .expect("a back-off");
}

fn hop_inside_its_delay(home: &std::path::Path, edge: &mut McpServer, base: &str) {
    hold_robots(
        home,
        &hop_url(base),
        "User-agent: *\nAllow: /\nCrawl-delay: 60\n",
    );
    edge.resolve = hop_at(address_of(base));
    let store = crate::crawl_delay::CrawlDelayStore::open(home);
    for _ in 0..2 {
        let turn = store.take_turn(
            HOP_HOST,
            crate::crawl_delay::WAIT_BUDGET,
            crate::crawl_delay::WAIT_BUDGET,
            Utc::now(),
            edge.pace,
        );
        assert!(turn.sends(), "{turn:?}");
    }
}

fn hop_disallowed(home: &std::path::Path, edge: &mut McpServer, base: &str) {
    hold_robots(
        home,
        &hop_url(base),
        "User-agent: *\nDisallow: /zqxvmarker-path\n",
    );
    edge.resolve = hop_at(address_of(base));
}

/// The hop's `robots.txt` disallows AI input in a `Content-Signal` line.
fn hop_signal_disallows_ai_input(home: &std::path::Path, edge: &mut McpServer, base: &str) {
    hold_robots(
        home,
        &hop_url(base),
        "User-agent: *\nAllow: /\nContent-Signal: ai-input=no\n",
    );
    edge.resolve = hop_at(address_of(base));
}

/// The hop's `robots.txt` disallows AI input in a `Content-Usage` rule
/// whose path is the marker's.
fn hop_usage_disallows_ai_input(home: &std::path::Path, edge: &mut McpServer, base: &str) {
    hold_robots(
        home,
        &hop_url(base),
        "User-agent: *\nAllow: /\nContent-Usage: /zqxvmarker-path ai-use=n\n",
    );
    edge.resolve = hop_at(address_of(base));
}

/// A publisher whose `robots.txt` redirects to a host this edge cannot
/// reach, so the file is unreachable and every path is disallowed.
fn robots_redirected_away(_: &str, target: &str) -> Vec<u8> {
    match target {
        "/robots.txt" => answer(
            &format!("HTTP/1.1 302 Found\r\nLocation: http://{HOP_HOST}/zqxvmarker-robots.txt\r\n"),
            b"",
        ),
        _ => page_ok(),
    }
}

/// A publisher whose `robots.txt` redirects within its origin to a file that
/// disallows the page: the file the record names is the redirect's target.
fn robots_redirected_within(_: &str, target: &str) -> Vec<u8> {
    match target {
        "/robots.txt" => answer(
            "HTTP/1.1 302 Found\r\nLocation: /zqxvmarker-robots.txt\r\n",
            b"",
        ),
        "/zqxvmarker-robots.txt" => answer(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n",
            b"User-agent: *\nDisallow: /page\n",
        ),
        _ => page_ok(),
    }
}

/// A publisher whose page names a licence at a host this edge cannot reach.
fn licence_unreachable(_: &str, target: &str) -> Vec<u8> {
    match target {
        "/robots.txt" => robots_allow(),
        _ => answer(
            &format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nLink: \
                 <http://{HOP_HOST}/zqxvmarker-licence.xml>; rel=license; \
                 type=\"application/rsl+xml\"\r\n"
            ),
            PAGE,
        ),
    }
}

/// A publisher whose licence is `licence`, named by every page.
fn licensed(licence: &'static str) -> fn(&str, &str) -> Vec<u8> {
    // A `fn` pointer cannot capture, so each licence has its own function.
    match licence {
        PAID_LICENCE => paid_site,
        SERVER_LICENCE => server_site,
        PROFILE_LICENCE => profile_site,
        AUDIT_LICENCE => audit_site,
        _ => unreachable!("a known licence"),
    }
}

const PAID_LICENCE: &str = r#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/"><license>
<permits type="usage">ai-input</permits><payment type="purchase"><standard>http://zqxvmarker-standard.invalid/s</standard><custom>http://zqxvmarker-custom.invalid/c</custom><amount currency="USD">1.00</amount></payment>
</license></content></rsl>"#;

const SERVER_LICENCE: &str = r#"<rsl xmlns="https://rslstandard.org/rsl"><content server="http://zqxvmarker-server.invalid/" url="/"><license>
<permits type="usage">ai-input</permits><payment type="attribution"/>
</license></content></rsl>"#;

const PROFILE_LICENCE: &str = r#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/"><license>
<permits type="usage">ai-input</permits><payment type="attribution"/>
<reporting type="telemetry" profile="https://zqxvmarker-profile.invalid/p" endpoint="https://zqxvmarker-endpoint.invalid/e"><![CDATA[{"conformance_level":"grounding"}]]></reporting>
</license></content></rsl>"#;

const AUDIT_LICENCE: &str = r#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/"><license>
<permits type="usage">ai-input</permits><payment type="attribution"/>
<reporting type="audit" profile="https://zqxvmarker-audit.invalid/p"/>
</license></content></rsl>"#;

fn licence_site(licence: &str, target: &str) -> Vec<u8> {
    match target {
        "/robots.txt" => robots_allow(),
        "/licence.xml" => answer(
            "HTTP/1.1 200 OK\r\nContent-Type: application/rsl+xml\r\n",
            licence.as_bytes(),
        ),
        _ => answer(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nLink: </licence.xml>; rel=license; \
             type=\"application/rsl+xml\"\r\n",
            PAGE,
        ),
    }
}

fn paid_site(_: &str, target: &str) -> Vec<u8> {
    licence_site(PAID_LICENCE, target)
}

fn server_site(_: &str, target: &str) -> Vec<u8> {
    licence_site(SERVER_LICENCE, target)
}

fn profile_site(_: &str, target: &str) -> Vec<u8> {
    licence_site(PROFILE_LICENCE, target)
}

fn audit_site(_: &str, target: &str) -> Vec<u8> {
    licence_site(AUDIT_LICENCE, target)
}

/// A publisher whose page disallows AI input in its `Content-Usage` header,
/// with the marker in a parameter of the same field.
fn usage_disallowed(_: &str, target: &str) -> Vec<u8> {
    match target {
        "/robots.txt" => robots_allow(),
        _ => raw_answer(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Usage: ai-use=n, \
              zqxvmarker=y\r\n",
            PAGE,
        ),
    }
}

/// A publisher whose page is a file this edge does not deliver, of a type
/// with the marker in it.
fn undelivered_file(_: &str, target: &str) -> Vec<u8> {
    match target {
        "/robots.txt" => robots_allow(),
        _ => answer(
            "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.ms-zqxvmarker\r\n",
            b"bytes",
        ),
    }
}

/// A publisher that answers 403 with a `cf-mitigated` value of its own.
fn challenged(_: &str, target: &str) -> Vec<u8> {
    match target {
        "/robots.txt" => robots_allow(),
        _ => answer(
            "HTTP/1.1 403 Forbidden\r\ncf-mitigated: zqxvmarker-mitigation\r\n",
            b"",
        ),
    }
}

/// A publisher whose `/page` redirects to [`hop_url`], which answers a bare
/// 404: no challenge header, a body of its own.
fn hop_answers_404(origin: &str, target: &str) -> Vec<u8> {
    match target {
        "/zqxvmarker-path" => answer(
            "HTTP/1.1 404 Not Found\r\nContent-Type: text/html\r\nX-Zqxvmarker: zqxvmarker-header\r\n",
            b"zqxvmarker body",
        ),
        _ => redirecting(origin, target),
    }
}

/// A publisher whose `/page` redirects to a URL whose scheme is not http.
fn redirect_to_data(_: &str, target: &str) -> Vec<u8> {
    match target {
        "/page" => answer(
            "HTTP/1.1 302 Found\r\nLocation: data:,zqxvmarker-data\r\n",
            b"",
        ),
        "/robots.txt" => robots_allow(),
        _ => page_ok(),
    }
}

/// A publisher whose `/page` redirects through six hops, past the limit.
fn redirect_loop(origin: &str, target: &str) -> Vec<u8> {
    match target {
        "/robots.txt" => robots_allow(),
        _ => answer(
            &format!("HTTP/1.1 302 Found\r\nLocation: {origin}{target}/zqxvmarker-loop\r\n"),
            b"",
        ),
    }
}

fn cases() -> Vec<Case> {
    let strict = |constraints: &str| {
        format!(
            r#"{{"policy_mode":"strict","allow_private_hosts":true,"constraints":[{constraints}]}}"#
        )
    };
    let observe = |constraints: &str| {
        format!(
            r#"{{"policy_mode":"observe","allow_private_hosts":true,"constraints":[{constraints}]}}"#
        )
    };
    let deny_hop = format!(r#"{{"kind":"denied_source_host","host":"{HOP_HOST}"}}"#);
    vec![
        Case {
            name: "redirect to a denied host",
            mode: "strict",
            policy: strict(&deny_hop),
            site: redirecting,
            arrange: nothing,
            recorded: &[HOP_HOST, "zqxvmarker-path"],
            told: "The job denies this source's host",
        },
        Case {
            name: "redirect whose robots.txt cannot be reached",
            mode: "strict",
            policy: strict(""),
            site: redirecting,
            arrange: nothing,
            recorded: &[HOP_HOST, "zqxvmarker-path"],
            told: "robots.txt could not be reached",
        },
        Case {
            name: "redirect whose robots.txt cannot be reached, observe",
            mode: "observe",
            policy: observe(""),
            site: redirecting,
            arrange: nothing,
            recorded: &[HOP_HOST, "zqxvmarker-path"],
            told: "robots.txt could not be reached",
        },
        Case {
            name: "redirect whose robots.txt disallows the target",
            mode: "strict",
            policy: strict(""),
            site: redirecting,
            arrange: hop_disallowed,
            recorded: &[HOP_HOST, "Disallow: /zqxvmarker-path"],
            told: "robots.txt disallows this fetcher at this path",
        },
        Case {
            name: "redirect whose robots.txt disallows the target, observe",
            mode: "observe",
            policy: observe(""),
            site: redirecting,
            arrange: hop_disallowed,
            recorded: &[HOP_HOST, "Disallow: /zqxvmarker-path"],
            told: "robots.txt disallows this fetcher at this path",
        },
        Case {
            name: "redirect whose robots.txt Content-Signal disallows AI input",
            mode: "strict",
            policy: strict(""),
            site: redirecting,
            arrange: hop_signal_disallows_ai_input,
            recorded: &[HOP_HOST, "Content-Signal: ai-input=no"],
            told: "The source disallows AI input (in a Content-Signal line in its robots.txt)",
        },
        Case {
            name: "redirect whose robots.txt Content-Signal disallows AI input, observe",
            mode: "observe",
            policy: observe(""),
            site: redirecting,
            arrange: hop_signal_disallows_ai_input,
            recorded: &[HOP_HOST, "Content-Signal: ai-input=no"],
            told: "The source disallows AI input (in a Content-Signal line in its robots.txt)",
        },
        Case {
            name: "redirect whose robots.txt Content-Usage rule disallows AI input",
            mode: "strict",
            policy: strict(""),
            site: redirecting,
            arrange: hop_usage_disallows_ai_input,
            recorded: &[HOP_HOST, "zqxvmarker-path ai-use=n"],
            told: "The source disallows AI input (in a Content-Usage rule in its robots.txt)",
        },
        Case {
            name: "redirect whose robots.txt Content-Usage rule disallows AI input, observe",
            mode: "observe",
            policy: observe(""),
            site: redirecting,
            arrange: hop_usage_disallows_ai_input,
            recorded: &[HOP_HOST, "zqxvmarker-path ai-use=n"],
            told: "The source disallows AI input (in a Content-Usage rule in its robots.txt)",
        },
        Case {
            name: "redirect that resolves into private space",
            mode: "strict",
            policy: r#"{"policy_mode":"strict","record_internal_prefixes":["{base}/"]}"#.to_owned(),
            site: redirecting,
            arrange: resolve_hop_to_private,
            recorded: &[HOP_HOST],
            told: "resolves to a local or private address",
        },
        Case {
            name: "redirect whose name does not resolve",
            mode: "strict",
            policy: strict(""),
            site: redirecting,
            arrange: |home, _, base| hold_robots(home, &hop_url(base), "User-agent: *\nAllow: /\n"),
            recorded: &[HOP_HOST],
            told: "its name could not be resolved",
        },
        Case {
            name: "redirect whose connection is refused",
            mode: "strict",
            policy: strict(""),
            site: redirecting,
            arrange: resolve_hop_to_closed_port,
            recorded: &[HOP_HOST],
            told: "could not be reached",
        },
        Case {
            name: "redirect to a host in back-off",
            mode: "strict",
            policy: strict(""),
            site: redirecting,
            arrange: hop_in_back_off,
            recorded: &[HOP_HOST],
            told: "is in back-off",
        },
        Case {
            name: "redirect to a host inside its Crawl-delay",
            mode: "strict",
            policy: strict(""),
            site: redirecting,
            arrange: hop_inside_its_delay,
            recorded: &[HOP_HOST],
            told: "The next request to the host may be sent",
        },
        Case {
            name: "redirect to a URL whose scheme is not http",
            mode: "strict",
            policy: strict(""),
            site: redirect_to_data,
            arrange: nothing,
            recorded: &["data:,zqxvmarker-data"],
            told: "scheme is not http or https",
        },
        Case {
            name: "redirect chain past the limit",
            mode: "strict",
            policy: strict(""),
            site: redirect_loop,
            arrange: nothing,
            recorded: &["zqxvmarker-loop"],
            told: "exceeded",
        },
        Case {
            name: "robots.txt redirected to an unreachable host",
            mode: "strict",
            policy: strict(""),
            site: robots_redirected_away,
            arrange: nothing,
            recorded: &["zqxvmarker-robots.txt"],
            told: "robots.txt could not be reached",
        },
        Case {
            name: "robots.txt redirected to an unreachable host, observe",
            mode: "observe",
            policy: observe(""),
            site: robots_redirected_away,
            arrange: nothing,
            recorded: &["zqxvmarker-robots.txt"],
            told: "robots.txt could not be reached",
        },
        Case {
            name: "robots.txt redirected within its origin to a Disallow",
            mode: "strict",
            policy: strict(""),
            site: robots_redirected_within,
            arrange: nothing,
            recorded: &["zqxvmarker-robots.txt"],
            told: "robots.txt disallows this fetcher at this path",
        },
        Case {
            name: "a licence the source names cannot be read",
            mode: "strict",
            policy: strict(""),
            site: licence_unreachable,
            arrange: nothing,
            recorded: &["zqxvmarker-licence.xml"],
            told: "could not be read",
        },
        Case {
            name: "a licence the source names cannot be read, observe",
            mode: "observe",
            policy: observe(""),
            site: licence_unreachable,
            arrange: nothing,
            recorded: &["zqxvmarker-licence.xml"],
            told: "could not be read",
        },
        Case {
            name: "a licence's payment term with standard and custom",
            mode: "strict",
            policy: strict(""),
            site: licensed(PAID_LICENCE),
            arrange: nothing,
            recorded: &["zqxvmarker-standard", "zqxvmarker-custom"],
            told: "payment term is unmet",
        },
        Case {
            name: "a licence's payment term with standard and custom, observe",
            mode: "observe",
            policy: observe(""),
            site: licensed(PAID_LICENCE),
            arrange: nothing,
            recorded: &["zqxvmarker-standard", "zqxvmarker-custom"],
            told: "payment term is unmet",
        },
        Case {
            name: "a licence's server",
            mode: "strict",
            policy: strict(""),
            site: licensed(SERVER_LICENCE),
            arrange: nothing,
            recorded: &["zqxvmarker-server"],
            told: "licence server",
        },
        Case {
            name: "a licence's server, observe",
            mode: "observe",
            policy: observe(""),
            site: licensed(SERVER_LICENCE),
            arrange: nothing,
            recorded: &["zqxvmarker-server"],
            told: "licence server",
        },
        Case {
            name: "a licence's reporting profile and endpoint",
            mode: "strict",
            policy: strict(""),
            site: licensed(PROFILE_LICENCE),
            arrange: nothing,
            recorded: &["zqxvmarker-profile", "zqxvmarker-endpoint"],
            told: "requires telemetry reporting",
        },
        Case {
            name: "a licence's reporting profile and endpoint, observe",
            mode: "observe",
            policy: observe(""),
            site: licensed(PROFILE_LICENCE),
            arrange: nothing,
            recorded: &["zqxvmarker-profile", "zqxvmarker-endpoint"],
            told: "requires telemetry reporting",
        },
        Case {
            name: "a licence's reporting of another type",
            mode: "strict",
            policy: strict(""),
            site: licensed(AUDIT_LICENCE),
            arrange: nothing,
            recorded: &["zqxvmarker-audit"],
            told: "requires reporting of type audit",
        },
        Case {
            name: "a licence's reporting of another type, observe",
            mode: "observe",
            policy: observe(""),
            site: licensed(AUDIT_LICENCE),
            arrange: nothing,
            recorded: &["zqxvmarker-audit"],
            told: "requires reporting of type audit",
        },
        Case {
            name: "a Content-Usage header that disallows AI input",
            mode: "strict",
            policy: strict(""),
            site: usage_disallowed,
            arrange: nothing,
            recorded: &["ai-use=n, zqxvmarker=y"],
            told: "disallows AI input",
        },
        Case {
            name: "a Content-Usage header that disallows AI input, observe",
            mode: "observe",
            policy: observe(""),
            site: usage_disallowed,
            arrange: nothing,
            recorded: &["ai-use=n, zqxvmarker=y"],
            told: "disallows AI input",
        },
        Case {
            name: "a file of a type this edge does not deliver",
            mode: "strict",
            policy: strict(""),
            site: undelivered_file,
            arrange: nothing,
            recorded: &["application/vnd.ms-zqxvmarker"],
            told: "a file this edge does not deliver",
        },
        Case {
            name: "a bare status at a redirect hop",
            mode: "strict",
            policy: strict(""),
            site: hop_answers_404,
            arrange: resolve_hop_to_publisher,
            recorded: &[HOP_HOST, "zqxvmarker-path"],
            told: "the target of redirect 1 answered 404",
        },
        Case {
            name: "a challenge the origin answers with",
            mode: "strict",
            policy: strict(""),
            site: challenged,
            arrange: nothing,
            recorded: &["zqxvmarker-mitigation"],
            told: "refused the request",
        },
    ]
}

/// EDG-129: for every refusal and failure path, in the mode that refuses
/// it, the agent reads nothing the source chose and the record keeps it.
#[test]
fn a_refused_crossing_gives_the_agent_no_text_from_the_source() {
    for case in cases() {
        let (base, _) = raw_site(case.site);
        let (home, mut edge) = server(&case.policy.replace("{base}", &base));
        (case.arrange)(home.path(), &mut edge, &base);
        let told = edge
            .tool_fetch(&json!({"url": format!("{base}/page")}))
            .expect_err(case.name);
        assert!(
            told.contains(case.told),
            "{} ({}): the agent reads why, in this edge's words: {told}",
            case.name,
            case.mode
        );
        // The header the publisher plants on every answer is in the record
        // wherever a page or a redirect was read; the case's own values are
        // checked by name.
        refused_without_source_text(home.path(), &told, case.recorded);
        eprintln!("{} ({}): {told}", case.name, case.mode);
    }
}

/// A hosted edge (`Pace::Hosted`) names each refusal and failure at a hop
/// by position, says whose rule it was, and names no file under the home.
#[test]
fn a_hosted_edge_refuses_at_a_hop_with_no_text_from_the_source() {
    let open = |mode: &str| format!(r#"{{"policy_mode":"{mode}","allow_private_hosts":true}}"#);
    let deny_hop = format!(
        r#"{{"policy_mode":"strict","allow_private_hosts":true,"constraints":[{{"kind":"denied_source_host","host":"{HOP_HOST}"}}]}}"#
    );
    let private_hop = r#"{"policy_mode":"strict","record_internal_prefixes":["{base}/"]}"#;
    type Site = fn(&str, &str) -> Vec<u8>;
    type Arrange = fn(&std::path::Path, &mut McpServer, &str);
    let cases: Vec<(&str, String, Site, Arrange, &str)> = vec![
        (
            "Content-Signal, strict",
            open("strict"),
            redirecting,
            hop_signal_disallows_ai_input,
            "(at the target of redirect 1; the source's terms)",
        ),
        (
            "Content-Signal, observe",
            open("observe"),
            redirecting,
            hop_signal_disallows_ai_input,
            "(at the target of redirect 1; the source's terms)",
        ),
        (
            "Disallow, observe",
            open("observe"),
            redirecting,
            hop_disallowed,
            "(at the target of redirect 1; the source's robots.txt)",
        ),
        (
            "Crawl-delay, strict",
            open("strict"),
            redirecting,
            hop_inside_its_delay,
            "(at the target of redirect 1; the source's robots.txt)",
        ),
        // A back-off is this edge's pacing, and names nobody.
        (
            "back-off, strict",
            open("strict"),
            redirecting,
            hop_in_back_off,
            "the host is in back-off (at the target of redirect 1)",
        ),
        (
            "a host the host list denies, strict",
            deny_hop,
            redirecting,
            resolve_hop_to_publisher,
            "(at the target of redirect 1; the operator's policy)",
        ),
        (
            "a private address the policy could admit, strict",
            private_hop.to_owned(),
            redirecting,
            resolve_hop_to_private,
            "(at the target of redirect 1; the operator's policy)",
        ),
        (
            "a bare status, strict",
            open("strict"),
            hop_answers_404,
            resolve_hop_to_publisher,
            "the target of redirect 1 answered 404",
        ),
    ];
    for (name, policy, site, arrange, ends) in cases {
        let (base, _) = raw_site(site);
        let (home, mut edge) = server(&policy.replace("{base}", &base));
        edge.pace = crate::crawl_delay::Pace::Hosted;
        arrange(home.path(), &mut edge, &base);
        let told = edge
            .tool_fetch(&json!({"url": format!("{base}/page")}))
            .expect_err(name);
        assert!(told.ends_with(ends), "{name}: {told}");
        assert!(
            !told.contains(&home.path().display().to_string()),
            "{name}: a hosted edge names no file under the home: {told}"
        );
        refused_without_source_text(home.path(), &told, &[HOP_HOST]);
        eprintln!("{name} (hosted): {told}");
    }
}

/// A name that resolves into private space is refused by this runtime where
/// a service-mode edge holds the private-address floor, which no policy
/// setting lifts, and by the operator's policy where the policy could admit
/// it. The sentence and its attribution agree in both.
#[test]
fn a_private_address_under_the_held_floor_is_the_runtimes_refusal() {
    let url = "https://zqxv-first.example/page";
    for held in [true, false] {
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            home.path().join("policy.json"),
            r#"{"policy_mode":"observe"}"#,
        )
        .expect("policy");
        hold_robots(home.path(), url, "User-agent: *\nAllow: /\n");
        let policy = SessionPolicy::load(home.path(), None).expect("the policy loads");
        let policy = if held {
            policy.hold_private_floor()
        } else {
            policy
        };
        let log = SessionLog::open(home.path(), "test-session").expect("session log");
        let credentials = commonmeasure_supply::credentials::CredentialsStatus {
            path: home
                .path()
                .join(commonmeasure_supply::credentials::CREDENTIALS_FILE),
            loaded: None,
        };
        let mut edge = McpServer::new(log, policy, "claude-code", None, credentials);
        edge.pace = crate::crawl_delay::Pace::Hosted;
        edge.resolve = Box::new(|_: &str| Ok(vec!["100.64.0.1:443".parse().expect("an address")]));
        let told = edge.tool_fetch(&json!({"url": url})).expect_err("refused");
        if held {
            assert!(
                told.ends_with("no policy setting lifts it here."),
                "the runtime's floor names nobody: {told}"
            );
            assert!(!told.contains("the operator's policy"), "{told}");
        } else {
            assert!(told.contains("record_internal_prefixes"), "{told}");
            assert!(told.ends_with("(the operator's policy)"), "{told}");
        }
        let crossing = crossings(home.path()).pop().expect("a crossing");
        assert_eq!(
            crossing["payload"]["told"].as_str(),
            Some(told.as_str()),
            "{crossing}"
        );
        assert_eq!(crossing["payload"]["grounded"], false, "{crossing}");
    }
}

/// The reporting route on an edge whose key the hub revoked, as the agent
/// reads it: the receiver by origin and digest, the licence's endpoint by
/// position and the hub by role. The record names the endpoint whole.
#[test]
fn the_revoked_key_arm_names_the_endpoint_by_position() {
    let home = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        home.path().join("policy.json"),
        r#"{"policy_mode":"strict"}"#,
    )
    .expect("policy");
    crate::consent::record(home.path(), crate::consent::Answer::Agreed, Utc::now())
        .expect("consent");
    std::fs::write(
        home.path().join("relay.json"),
        json!({"receiver": "https://hub.example/api/v1/telemetry"}).to_string(),
    )
    .expect("relay.json");
    std::fs::write(
        crate::enrolment::EnrolmentRecord::path(home.path()),
        json!({
            "hub": "https://hub.example/", "organization": {"id": "org", "name": "Org"},
            "name": "edge", "key_id": "key",
            "identity": {"origin": "https://hub.example", "bot_page": "https://hub.example/bot"},
            "enrolled_at": "2026-09-30T00:00:00Z",
            "revocation_learnt_at": "2026-09-30T01:00:00Z"
        })
        .to_string(),
    )
    .expect("enrolment.json");
    for pace in [
        crate::crawl_delay::Pace::Own,
        crate::crawl_delay::Pace::Hosted,
    ] {
        let mut edge = server_at(home.path());
        edge.pace = pace;
        let ruling = edge.interval_relay().reporting_ruling_for(
            "https://zqxvmarker.example/article",
            Some("zqxvmarker-profile".to_owned()),
            None,
            Some("https://zqxvmarker-endpoint.example/events"),
            None,
        );
        let agent = ruling
            .agent_reason
            .expect("an agent sentence")
            .into_string();
        assert!(
            !agent.to_ascii_lowercase().contains(MARKER),
            "{pace:?}: {agent}"
        );
        assert!(
            agent.contains("the endpoint the licence names"),
            "{pace:?}: {agent}"
        );
        assert!(agent.contains("is revoked"), "{pace:?}: {agent}");
        assert!(!agent.contains("hub.example/api"), "{pace:?}: {agent}");
        let recorded = ruling.reason.expect("a record sentence");
        assert!(
            recorded.contains("https://zqxvmarker-endpoint.example/events"),
            "{pace:?}: the record names the endpoint whole: {recorded}"
        );
    }
}

/// EDG-129: a refused search result gives the agent its position and the
/// reason, and neither its URL, its title nor its snippet; the record keeps
/// all three.
#[test]
fn a_refused_search_result_gives_the_agent_no_text_from_the_supplier() {
    let (home, mut edge) = server(
        r#"{"policy_mode":"strict","constraints":[{"kind":"denied_source_host","host":"zqxvmarker-search.invalid"}]}"#,
    );
    let mut refused = envelope_for("https://zqxvmarker-search.invalid/zqxvmarker-path");
    refused.title = Some("zqxvmarker-title".to_owned());
    refused.text = Some("zqxvmarker-snippet".to_owned());
    refused.retrieval_rank = 2;
    let mut admitted = envelope_for("https://publisher.example/page");
    admitted.title = Some("A page".to_owned());
    admitted.text = Some("Its text.".to_owned());
    let acquisition = Acquisition {
        provider: "exa".to_owned(),
        capability: commonmeasure_types::ProviderCapability::Search,
        endpoint: "https://api.exa.ai/search".to_owned(),
        http_status: Some(200),
        latency_ms: 1,
        provider_request_id: None,
        charge: commonmeasure_types::AcquisitionCharge::default(),
        envelopes: vec![admitted, refused],
        raw_response: Vec::new(),
        invocation: None,
    };
    let delivered = edge.deliver(&acquisition);
    let told = delivered.to_string();
    assert!(
        !told.to_ascii_lowercase().contains(MARKER),
        "the agent read a planted value: {told}"
    );
    assert_eq!(delivered["refused"], 1, "{delivered}");
    assert_eq!(delivered["refusals"][0]["position"], 2, "{delivered}");
    assert_eq!(delivered["refusals"][0]["of"], 2, "{delivered}");
    assert!(
        delivered["refusals"][0]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("The job denies this source's host")),
        "{delivered}"
    );
    let crossing = crossings(home.path())
        .pop()
        .expect("the refused result is recorded");
    assert_eq!(crossing["payload"]["grounded"], false, "{crossing}");
    let record = crossing.to_string();
    for value in ["zqxvmarker-search.invalid", "zqxvmarker-path"] {
        assert!(record.contains(value), "the record keeps {value}: {record}");
    }
    assert!(
        crossing["payload"]["refusal"]
            .as_str()
            .is_some_and(|reason| reason.contains("zqxvmarker-search.invalid")),
        "the record's sentence names the host whole: {crossing}"
    );
    // The refused result's record says what the agent read of it: the
    // reason, and the position beside it.
    assert_eq!(
        crossing["payload"]["told"], delivered["refusals"][0]["reason"],
        "{crossing}"
    );
    assert_eq!(
        crossing["payload"]["told_position"],
        json!({"position": 2, "of": 2}),
        "{crossing}"
    );
    told_holds_no_source_value_of(&crossing);
}

/// A delivered page records no `told`: the result itself is what the agent
/// read, and the record claims nothing more about it.
#[test]
fn a_delivered_page_records_nothing_told() {
    fn serving(_: &str, target: &str) -> Vec<u8> {
        match target {
            "/robots.txt" => robots_allow(),
            _ => page_ok(),
        }
    }
    let (base, _) = raw_site(serving);
    let (home, mut edge) = server(r#"{"policy_mode":"strict","allow_private_hosts":true}"#);
    edge.tool_fetch(&json!({"url": format!("{base}/page")}))
        .expect("delivered");
    let crossing = crossings(home.path()).pop().expect("a crossing");
    assert_eq!(crossing["event"], "crossing_mediated", "{crossing}");
    assert_eq!(crossing["payload"]["grounded"], true, "{crossing}");
    assert!(crossing["payload"]["told"].is_null(), "{crossing}");
}
