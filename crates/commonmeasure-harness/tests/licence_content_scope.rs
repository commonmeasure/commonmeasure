//! Which `<content>` entry of an RSL licence governs a page, when the entry
//! names an absolute URL: read as a crossing reads it, through the reader
//! that runs before a page is requested, the declaration cache and the real
//! HTTP client, from a loopback publisher.
//!
//! The page URLs name `publisher.example` on the scheme's default port, which
//! a loopback test cannot listen on, so the probe sends each request for that
//! origin to the loopback publisher instead. Everything from the page URL to
//! the terms is the production path; only the socket address is substituted.

use std::sync::{Arc, Mutex};

use chrono::Utc;
use commonmeasure_harness::declarations::Category;
use commonmeasure_harness::discovery::{self, DeclarationCache, Declarations};
use commonmeasure_http::{Request, Response, Server, ServerHandle};

const ROBOTS: &str = "License: /license.xml\nUser-agent: *\nAllow: /\n";

/// A licence whose one `<content>` entry names `scope`, prohibits AI input
/// and demands usage reporting.
fn licence(scope: &str) -> String {
    format!(
        r#"<rsl xmlns="https://rslstandard.org/rsl">{}</rsl>"#,
        prohibiting(scope)
    )
}

/// A `<content>` entry that names `scope`, prohibits AI input and demands
/// usage reporting.
fn prohibiting(scope: &str) -> String {
    format!(
        r#"
  <content url="{scope}">
    <license>
      <prohibits type="usage">ai-input</prohibits>
      <reporting type="telemetry" profile="https://contenttelemetry.org/profiles/spur">
        <![CDATA[{{"conformance_level": "grounding"}}]]>
      </reporting>
    </license>
  </content>
"#
    )
}

/// A `<content>` entry that names `scope`, permits AI input and demands
/// nothing.
fn permitting(scope: &str) -> String {
    format!(
        r#"
  <content url="{scope}"><license><permits type="usage">ai-input</permits></license></content>
"#
    )
}

/// The publisher, serving `robots.txt` and a licence scoped to `scope`, and
/// every path it was asked for.
fn publisher(scope: &str) -> (ServerHandle, Arc<Mutex<Vec<String>>>) {
    publishing(licence(scope))
}

/// The publisher, serving `robots.txt` and the licence `body`, and every
/// path it was asked for.
fn publishing(body: String) -> (ServerHandle, Arc<Mutex<Vec<String>>>) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&asked);
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |request| {
            seen.lock().unwrap().push(request.target.clone());
            match request.target.as_str() {
                "/robots.txt" => Response::text(200, ROBOTS),
                "/license.xml" => {
                    let mut response = Response::new(200, body.as_bytes().to_vec());
                    response.headers.set("Content-Type", "application/rsl+xml");
                    response
                }
                _ => Response::text(404, "no such file"),
            }
        })
        .expect("spawn");
    (handle, asked)
}

/// What the reader establishes for `page` before the page is requested.
fn before_fetch(site: &ServerHandle, page: &str) -> Declarations {
    let home = tempfile::tempdir().expect("tempdir");
    let loopback = site.url();
    let probe = |url: &str,
                 _: discovery::Redirects|
     -> Result<(String, Response), discovery::ProbeFailure> {
        let parsed = url::Url::parse(url).expect("the reader asks absolute URLs");
        assert_eq!(
            parsed.host_str().map(|host| host.trim_end_matches('.')),
            Some("publisher.example"),
            "{url}"
        );
        let routed = format!("{loopback}{}", parsed.path());
        commonmeasure_http::send(&routed, Request::get("/"))
            .map(|response| (url.to_owned(), response))
            .map_err(|error| discovery::ProbeFailure::Unreachable(error.to_string()))
    };
    discovery::before_fetch(
        &DeclarationCache::open(home.path()),
        page,
        None,
        Utc::now(),
        &probe,
        None,
        commonmeasure_types::PolicyMode::Strict,
    )
}

/// The licence governs the page: its prohibition and its reporting demand
/// are both in the declarations the ruling is made from.
fn assert_governed(page: &str, scope: &str, declarations: &Declarations) {
    let (licence, terms) = declarations
        .licence_terms()
        .unwrap_or_else(|| panic!("{scope} governs {page}: {declarations:#?}"));
    assert_eq!(licence.content.as_deref(), Some(scope), "{page}");
    assert!(
        terms
            .statements
            .iter()
            .any(|statement| statement.category == Category::AiInput
                && statement.preference
                    == commonmeasure_harness::declarations::Preference::Disallow),
        "{page}: {terms:?}"
    );
    assert_eq!(
        terms.reporting[0].profile, "https://contenttelemetry.org/profiles/spur",
        "{page}"
    );
}

/// Each page spelling against each scope spelling: the scheme's default
/// port written out or left implied, a trailing dot and a change of case,
/// on either side.
#[test]
fn an_absolute_scope_governs_every_spelling_of_a_page_under_it() {
    for (scope, page) in [
        (
            "https://publisher.example/",
            "https://publisher.example:443/story",
        ),
        (
            "https://publisher.example:443/",
            "https://publisher.example/story",
        ),
        (
            "http://publisher.example/",
            "http://publisher.example:80/story",
        ),
        (
            "http://publisher.example:80/",
            "http://publisher.example/story",
        ),
        (
            "https://publisher.example/",
            "https://publisher.example./story",
        ),
        (
            "https://publisher.example./",
            "https://publisher.example/story",
        ),
        (
            "https://publisher.example/",
            "https://PUBLISHER.Example/story",
        ),
        (
            "https://Publisher.Example.:443/",
            "https://publisher.example/story",
        ),
    ] {
        let (site, asked) = publisher(scope);
        let declarations = before_fetch(&site, page);
        assert_eq!(
            *asked.lock().unwrap(),
            ["/robots.txt", "/license.xml"],
            "{page}: the licence is read before the page"
        );
        assert_governed(page, scope, &declarations);
    }
}

/// The scope keeps the scheme, a port other than the default and the path:
/// a page outside it is not governed by it, however its host is spelled.
#[test]
fn an_absolute_scope_does_not_govern_another_scheme_port_or_path() {
    for (scope, page) in [
        (
            "https://publisher.example/",
            "http://publisher.example./story",
        ),
        (
            "https://publisher.example:8443/",
            "https://publisher.example./story",
        ),
        (
            "https://publisher.example/news/",
            "https://PUBLISHER.EXAMPLE./sport/1",
        ),
    ] {
        let (site, _) = publisher(scope);
        let declarations = before_fetch(&site, page);
        assert!(
            declarations.licence_terms().is_none(),
            "{scope} does not govern {page}: {declarations:#?}"
        );
        assert_eq!(
            declarations.licences[0].unavailable.as_deref(),
            Some("no <content> entry in the licence matches this page"),
            "{page}"
        );
    }
}

/// A relative and an absolute scope govern every spelling of a path under
/// them: a percent-encoded unreserved character, lower-case hex, a literal
/// `*` the scope names as `%2A`, or a fragment, on either side, leaves the
/// licence's prohibition and reporting demand in the ruling. So does a
/// doubled slash or an encoded `/` in the page's path, which a server may
/// serve as the path under the scope, when no entry covers the page as
/// parsed. An anchored scope covers a path that ends with its suffix
/// wherever else the suffix occurs.
#[test]
fn a_scope_governs_every_path_spelling_of_a_page_under_it() {
    for (scope, page) in [
        ("/news/", "https://publisher.example/%6Eews/1"),
        ("/news/", "https://publisher.example/%6eews/1"),
        ("/news/", "https://publisher.example//news/1"),
        ("/news/", "https://publisher.example/news%2F1"),
        ("/news/", "https://publisher.example/x/..%2Fnews/1"),
        ("/%6Eews/", "https://publisher.example/news/1"),
        ("/*.pdf$", "https://publisher.example/a.pdf/b.pdf"),
        ("/a/b%2Fc", "https://publisher.example/a//b%2Fc"),
        ("/file-%2A.html", "https://publisher.example/file-*.html"),
        (
            "https://publisher.example/news/",
            "https://publisher.example/%6Eews/1",
        ),
        (
            "https://publisher.example/news/",
            "https://publisher.example//news/1",
        ),
        (
            "https://publisher.example/news/",
            "https://publisher.example/news%2F1",
        ),
        (
            "https://publisher.example/news/",
            "https://publisher.example/x/..%2Fnews/1",
        ),
        (
            "https://publisher.example/%6eews/",
            "https://publisher.example/news/1",
        ),
        (
            "https://publisher.example/news/#part",
            "https://publisher.example/news/1",
        ),
    ] {
        let (site, asked) = publisher(scope);
        let declarations = before_fetch(&site, page);
        assert_eq!(
            *asked.lock().unwrap(),
            ["/robots.txt", "/license.xml"],
            "{page}: the licence is read before the page"
        );
        assert_governed(page, scope, &declarations);
    }
}

/// A scope names the path its author wrote: its own `//` or `%2F` is never
/// merged or decoded, so it does not reach a page it does not name. A page's
/// encoded `/` is decoded only to find the path a server may serve, never
/// to widen a scope beyond that path.
#[test]
fn a_scope_does_not_govern_a_page_outside_the_path_it_names() {
    for (scope, page) in [
        ("/news/1/", "https://publisher.example/news%2F1"),
        ("//news/", "https://publisher.example/news/1"),
        ("/a%2Fb", "https://publisher.example/a/b"),
        (
            "https://publisher.example/news/1/",
            "https://publisher.example/news%2F1",
        ),
        (
            "https://publisher.example//news/",
            "https://publisher.example/news/1",
        ),
    ] {
        let (site, _) = publisher(scope);
        let declarations = before_fetch(&site, page);
        assert!(
            declarations.licence_terms().is_none(),
            "{scope} does not govern {page}: {declarations:#?}"
        );
    }
}

/// Where a page's readings select different entries, the entry the page as
/// parsed selects governs: `/a//b` stays under the broad `/a/` entry and its
/// prohibition, as up to 0.4.2, and is not moved to the narrower `/a/b`
/// entry that permits AI input.
#[test]
fn the_entry_the_page_as_parsed_selects_governs_over_a_merged_reading() {
    for (broad, narrow) in [
        ("/a/", "/a/b"),
        (
            "https://publisher.example/a/",
            "https://publisher.example/a/b",
        ),
    ] {
        let body = format!(
            r#"<rsl xmlns="https://rslstandard.org/rsl">{}{}</rsl>"#,
            prohibiting(broad),
            permitting(narrow)
        );
        let page = "https://publisher.example/a//b";
        let (site, _) = publishing(body);
        assert_governed(page, broad, &before_fetch(&site, page));
    }
}

/// A scope governs a page that decoding `%2F`, merging `/` and resolving
/// dot segments reach in any order: nginx serves `/x//..%2Fnews/1` as
/// `/news/1`, merging before it resolves, so the `/news/` entry's
/// prohibition and reporting demand bind where no entry covers the page as
/// parsed. A merging proxy in front of a server that decodes and resolves
/// reaches `/a/news/1` from `/a/b//..%2F%2F..%2Fnews/1`.
#[test]
fn a_scope_governs_a_page_any_order_of_the_operations_reaches() {
    for (scope, page) in [
        ("/news/", "https://publisher.example/x//..%2Fnews/1"),
        ("/x/news/", "https://publisher.example/x//..%2Fnews/1"),
        (
            "/a/news/",
            "https://publisher.example/a/b//..%2F%2F..%2Fnews/1",
        ),
        (
            "https://publisher.example/news/",
            "https://publisher.example/x//..%2Fnews/1",
        ),
    ] {
        let (site, asked) = publisher(scope);
        let declarations = before_fetch(&site, page);
        assert_eq!(
            *asked.lock().unwrap(),
            ["/robots.txt", "/license.xml"],
            "{page}: the licence is read before the page"
        );
        assert_governed(page, scope, &declarations);
    }
}

/// A scope governs a page that stripping `;` path parameters or reading
/// `%5C` as `/` brings under it, as Tomcat serves `/news;x/1` and IIS reads
/// `/news%5C1` as `/news/1`, where no entry covers the page as parsed. An
/// encoded `;` is read as `;` and stripped, as nginx in front of Tomcat
/// serves `/news%3Bx/1` as `/news/1`.
#[test]
fn a_scope_governs_a_page_a_stripped_parameter_or_a_decoded_backslash_reaches() {
    for page in [
        "https://publisher.example/news;x/1",
        "https://publisher.example/x/..;/news/1",
        "https://publisher.example/news%5C1",
        "https://publisher.example/news%3Bx/1",
    ] {
        let (site, asked) = publisher("/news/");
        let declarations = before_fetch(&site, page);
        assert_eq!(
            *asked.lock().unwrap(),
            ["/robots.txt", "/license.xml"],
            "{page}: the licence is read before the page"
        );
        assert_governed(page, "/news/", &declarations);
    }
}

/// Where no entry covers the page as parsed, the first reading in
/// breadth-first order that selects an entry governs: `/x//..%2Fnews/1`
/// reaches `/x/news/1` in two steps (decode, then resolve) and `/news/1`
/// in three (decode, merge, resolve), so `/x/news/` governs over `/news/`.
#[test]
fn the_entry_the_nearest_reading_selects_governs_over_a_farther_one() {
    let body = format!(
        r#"<rsl xmlns="https://rslstandard.org/rsl">{}{}</rsl>"#,
        permitting("/news/"),
        prohibiting("/x/news/"),
    );
    let page = "https://publisher.example/x//..%2Fnews/1";
    let (site, _) = publishing(body);
    assert_governed(page, "/x/news/", &before_fetch(&site, page));
}
