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
        r#"<rsl xmlns="https://rslstandard.org/rsl">
  <content url="{scope}">
    <license>
      <prohibits type="usage">ai-input</prohibits>
      <reporting type="telemetry" profile="https://contenttelemetry.org/profiles/spur">
        <![CDATA[{{"conformance_level": "grounding"}}]]>
      </reporting>
    </license>
  </content>
</rsl>"#
    )
}

/// The publisher, serving `robots.txt` and a licence scoped to `scope`, and
/// every path it was asked for.
fn publisher(scope: &str) -> (ServerHandle, Arc<Mutex<Vec<String>>>) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&asked);
    let body = licence(scope);
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
