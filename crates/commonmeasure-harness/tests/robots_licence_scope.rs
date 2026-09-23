//! Where a `robots.txt` `License:` line applies, read as a crossing reads it:
//! from a loopback publisher over the real HTTP client, through the
//! declaration cache and the reader that runs before a page is requested.
//! Publishers place a site-wide licence after the last group, separated by a
//! blank line or following a `Sitemap:` line; a licence among a group's
//! lines is that group's alone.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::Utc;
use commonmeasure_harness::declarations::PRODUCT_TOKEN;
use commonmeasure_harness::discovery::{
    self, CacheDecision, DeclarationCache, Declarations, LicenceMechanism,
};
use commonmeasure_http::{Request, Response, Server, ServerHandle};

const LICENCE: &str = r#"<rsl xmlns="https://rslstandard.org/rsl">
  <content url="/">
    <license>
      <permits type="usage">ai-input</permits>
      <payment type="use">
        <amount currency="USD">0.015</amount>
      </payment>
      <reporting type="telemetry" profile="https://contenttelemetry.org/profiles/spur"
                 endpoint="https://telemetry.example.com/v1/events">
        <![CDATA[{"conformance_level": "grounding", "privacy_level": "minimal"}]]>
      </reporting>
    </license>
  </content>
</rsl>"#;

/// The site-wide licence after the last group, which is not the `*` group,
/// and a blank line.
const AFTER_LAST_GROUP: &str = "\
User-agent: *
Allow: /

User-agent: SomeOtherBot
Disallow: /

License: /license.xml
";

/// The site-wide licence after a `Sitemap:` line, with no blank line.
const AFTER_SITEMAP: &str = "\
User-agent: *
Disallow: /m/
Sitemap: /sitemap.xml
License: /license.xml
";

/// A licence among the `*` group's lines, and a second group without one.
const INSIDE_STAR_GROUP: &str = "\
User-agent: *
Allow: /
License: /license.xml

User-agent: SomeOtherBot
Disallow: /
";

/// A licence among another group's lines only: the `*` group reads none.
const INSIDE_OTHER_GROUP: &str = "\
User-agent: *
Allow: /

User-agent: SomeOtherBot
Disallow: /
License: /license.xml
";

struct Publisher {
    handle: ServerHandle,
    licence_hits: Arc<AtomicUsize>,
}

fn publisher(robots: &'static str) -> Publisher {
    let licence_hits = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&licence_hits);
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |request| match request.target.as_str() {
            "/robots.txt" => Response::text(200, robots),
            "/license.xml" => {
                count.fetch_add(1, Ordering::SeqCst);
                let mut response = Response::new(200, LICENCE.as_bytes().to_vec());
                response.headers.set("Content-Type", "application/rsl+xml");
                response
            }
            _ => Response::text(404, "no such file"),
        })
        .expect("spawn");
    Publisher {
        handle,
        licence_hits,
    }
}

/// What the reader establishes for `path` at the publisher before the page
/// is requested, fetching over loopback.
fn before_fetch(site: &Publisher, home: &std::path::Path, path: &str) -> Declarations {
    let probe = |url: &str| -> Result<(String, Response), discovery::ProbeFailure> {
        commonmeasure_http::send(url, Request::get("/"))
            .map(|response| (url.to_owned(), response))
            .map_err(|error| discovery::ProbeFailure::Unreachable(error.to_string()))
    };
    let page = format!("{}{path}", site.handle.url());
    discovery::before_fetch(
        &DeclarationCache::open(home),
        &page,
        None,
        Utc::now(),
        &probe,
        None,
        commonmeasure_types::PolicyMode::Observe,
    )
}

fn assert_site_licence_read(site: &Publisher, declarations: &Declarations) {
    let reading = declarations
        .robots
        .reading
        .as_ref()
        .expect("robots.txt was read");
    assert_eq!(reading.group.as_deref(), Some("*"));
    assert_eq!(reading.licences, ["/license.xml"]);
    let licence = declarations
        .licences
        .first()
        .expect("the licence named by robots.txt is read before the request");
    assert_eq!(licence.mechanism, LicenceMechanism::RobotsLicense);
    assert_eq!(licence.url, format!("{}/license.xml", site.handle.url()));
    assert_eq!(licence.cache, CacheDecision::Fetched);
    let terms = licence
        .terms
        .as_ref()
        .expect("the licence's terms are read");
    assert_eq!(
        terms.reporting[0].profile, "https://contenttelemetry.org/profiles/spur",
        "the reporting demand reaches the record"
    );
    assert_eq!(site.licence_hits.load(Ordering::SeqCst), 1);
}

#[test]
fn a_licence_after_the_last_group_and_a_blank_line_is_read_for_the_star_group() {
    let site = publisher(AFTER_LAST_GROUP);
    let home = tempfile::tempdir().expect("tempdir");
    let declarations = before_fetch(&site, home.path(), "/news/1");
    assert_site_licence_read(&site, &declarations);
}

#[test]
fn a_licence_after_a_sitemap_line_is_read_for_the_star_group() {
    let site = publisher(AFTER_SITEMAP);
    let home = tempfile::tempdir().expect("tempdir");
    let declarations = before_fetch(&site, home.path(), "/story");
    assert_site_licence_read(&site, &declarations);
    let reading = declarations.robots.reading.as_ref().unwrap();
    assert_eq!(reading.crawlable, Some(true));
}

#[test]
fn a_licence_inside_a_group_is_read_for_that_group_alone() {
    let own = publisher(INSIDE_STAR_GROUP);
    let home = tempfile::tempdir().expect("tempdir");
    let declarations = before_fetch(&own, home.path(), "/news/1");
    assert_site_licence_read(&own, &declarations);

    let other = publisher(INSIDE_OTHER_GROUP);
    let home = tempfile::tempdir().expect("tempdir");
    let declarations = before_fetch(&other, home.path(), "/news/1");
    let reading = declarations.robots.reading.as_ref().unwrap();
    assert_eq!(reading.group.as_deref(), Some("*"));
    assert!(
        reading.licences.is_empty(),
        "another group's licence is not the {PRODUCT_TOKEN} group's"
    );
    assert!(declarations.licences.is_empty());
    assert_eq!(other.licence_hits.load(Ordering::SeqCst), 0);
}
