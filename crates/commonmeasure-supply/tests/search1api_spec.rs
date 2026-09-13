//! Search1API, over the real transport, against its documented response shape.
//!
//! Search1API's verification state and the artefacts behind it are recorded
//! in `docs/knowledge-base/provider-verification.md`; no recon capture is
//! committed, so these tests need no credential. Each test starts a loopback origin
//! serving a documented-shape body, points the adapter's base URL at it, and
//! asserts on both directions — the request the adapter put on the wire, and
//! the envelopes and charge its parser derived from the reply. Nothing in the
//! path is a stand-in but the origin, which is the substitution `AGENTS.md`
//! permits.

use commonmeasure_http::Response;
use commonmeasure_supply::{Search1ApiAdapter, SupplyAdapter, SupplyError};
use commonmeasure_types::{ChargeBasis, LicenceState, ProviderCapability};
use serde_json::Value;

mod common;

use common::Origin;

/// The documented response shape, with two target-publisher results. The first
/// carries a crawled `content` body; the second only a `snippet`, to prove the
/// parser prefers the body where present and falls back to the snippet where it
/// is not.
fn documented_response() -> Value {
    serde_json::json!({
        "searchParameters": {
            "query": "EU AI Act code of practice",
            "search_service": "google",
            "max_results": 5,
            "crawl_results": 0
        },
        "results": [
            {
                "title": "AI Act enters force",
                "link": "https://www.theguardian.com/technology/2026/aug/01/ai-act",
                "snippet": "A short search excerpt about the Act.",
                "content": "The full crawled body of the Guardian article."
            },
            {
                "title": "Brussels briefing",
                "link": "https://www.ft.com/content/abc-123",
                "snippet": "The FT search excerpt.",
                "content": ""
            }
        ],
        "images": []
    })
}

/// The request must reach the documented endpoint with Bearer auth, carry the
/// query and the clamped result count, and the reply must parse into one
/// envelope per result — the first taking its text from the crawled `content`,
/// the second falling back to its `snippet`. The charge is the published
/// one-credit price, marked quoted because the response discloses no cost.
#[test]
fn search1api_search_parses_its_documented_shape() {
    let origin = Origin::serving(|_| {
        Response::new(200, serde_json::to_vec(&documented_response()).unwrap())
    });

    let acquisition = Search1ApiAdapter::new(&origin.url(), "test-key")
        .search("EU AI Act code of practice", 5, &[])
        .expect("documented shape");

    let request = origin.only_request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/search");
    assert_eq!(request.header("Authorization"), Some("Bearer test-key"));
    assert_eq!(request.json_body()["query"], "EU AI Act code of practice");
    assert_eq!(request.json_body()["search_service"], "google");
    assert_eq!(request.json_body()["max_results"], 5);

    assert_eq!(acquisition.provider, "search1api");
    assert_eq!(acquisition.http_status, Some(200));
    // The documented response carries no request or search identifier.
    assert!(acquisition.provider_request_id.is_none());
    assert_eq!(acquisition.envelopes.len(), 2);

    let first = &acquisition.envelopes[0];
    assert_eq!(first.host, "www.theguardian.com");
    assert_eq!(
        first.source_url,
        "https://www.theguardian.com/technology/2026/aug/01/ai-act"
    );
    assert_eq!(first.title.as_deref(), Some("AI Act enters force"));
    assert_eq!(first.retrieval_rank, 1);
    // A populated `content` body is preferred over the snippet, and hashed.
    assert_eq!(
        first.text.as_deref(),
        Some("The full crawled body of the Guardian article.")
    );
    assert!(
        first
            .content_hash
            .as_deref()
            .unwrap()
            .starts_with("sha256:")
    );
    // No Search1API response carries a licence field, and no date field either.
    assert_eq!(first.licence, LicenceState::Unknown);
    assert!(first.declared_date.is_none());
    // Promoted fields do not reappear under their own names.
    assert!(first.native_metadata.get("link").is_none());
    assert!(first.native_metadata.get("snippet").is_none());
    assert!(first.native_metadata.get("content").is_none());

    // The second result has an empty `content`, so its text falls back to the
    // snippet rather than being recorded as empty.
    let second = &acquisition.envelopes[1];
    assert_eq!(second.host, "www.ft.com");
    assert_eq!(second.text.as_deref(), Some("The FT search excerpt."));

    // No cost field on the response: the published one-credit price, quoted.
    assert!(acquisition.charge.money.is_none());
    let native = acquisition
        .charge
        .native
        .expect("a quoted charge is recorded");
    assert_eq!(native.unit, "credits");
    assert_eq!(native.amount.as_u64(), Some(1));
    assert_eq!(native.basis, ChargeBasis::Quoted);
    assert!(native.note.is_some(), "a quoted charge must say why");
}

/// A job's allowed hosts scope the Search1API search through `include_sites`;
/// with none named the field is absent entirely, so an ordinary open-web run is
/// byte-identical to before.
#[test]
fn search1api_scopes_the_search_to_the_jobs_allowed_hosts_and_omits_the_field_when_there_are_none()
{
    let scoped = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({"results": []})).unwrap(),
        )
    });
    Search1ApiAdapter::new(&scoped.url(), "test-key")
        .search("anything", 2, &["theguardian.com", "ft.com"])
        .expect("documented shape");
    assert_eq!(
        scoped.only_request().json_body()["include_sites"],
        serde_json::json!(["theguardian.com", "ft.com"]),
        "named hosts must reach Search1API as include_sites"
    );

    let unscoped = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({"results": []})).unwrap(),
        )
    });
    Search1ApiAdapter::new(&unscoped.url(), "test-key")
        .search("anything", 2, &[])
        .expect("documented shape");
    assert!(
        unscoped
            .only_request()
            .json_body()
            .get("include_sites")
            .is_none(),
        "an unscoped search must not send include_sites at all"
    );
}

/// Search1API's `fetch` capability is `/crawl`: it names one URL and returns
/// the full page body as `results.content` (a single object, not an array).
#[test]
fn search1api_fetch_crawls_a_named_url() {
    let origin = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({
                "crawlParameters": {"url": "https://www.ft.com/content/x"},
                "results": {
                    "title": "A headline",
                    "link": "https://www.ft.com/content/x",
                    "content": "The full crawled article body.",
                    "metadata": {"sourceUrl": "https://www.ft.com/content/x", "availability": "full"}
                }
            }))
            .unwrap(),
        )
    });
    let acquisition = Search1ApiAdapter::new(&origin.url(), "test-key")
        .fetch("https://www.ft.com/content/x")
        .expect("documented shape");
    let request = origin.only_request();
    assert_eq!(request.target, "/crawl");
    assert_eq!(request.header("Authorization"), Some("Bearer test-key"));
    assert_eq!(request.json_body()["url"], "https://www.ft.com/content/x");
    assert_eq!(acquisition.capability, ProviderCapability::Fetch);
    assert_eq!(acquisition.envelopes.len(), 1);
    assert_eq!(acquisition.envelopes[0].host, "www.ft.com");
    assert_eq!(
        acquisition.envelopes[0].text.as_deref(),
        Some("The full crawled article body.")
    );
    let native = acquisition.charge.native.expect("crawl is billable");
    assert_eq!(native.basis, ChargeBasis::Quoted);
}

/// A `200` whose body lacks `results` is a changed shape, not an empty
/// search or a crawl that found nothing; the error names the field and no
/// acquisition (so no charge) exists.
#[test]
fn search1api_treats_a_200_without_results_as_an_error_naming_the_field() {
    let origin = Origin::serving(|_| Response::new(200, b"{}".to_vec()));
    let error = Search1ApiAdapter::new(&origin.url(), "test-key")
        .search("anything", 1, &[])
        .expect_err("a body without `results` must fail");
    assert!(matches!(error, SupplyError::Malformed { .. }), "{error:?}");
    assert!(error.to_string().contains("results"), "{error}");

    let origin = Origin::serving(|_| Response::new(200, b"{}".to_vec()));
    let error = Search1ApiAdapter::new(&origin.url(), "test-key")
        .fetch("https://www.bbc.co.uk/news/x")
        .expect_err("a crawl body without `results` must fail");
    assert!(matches!(error, SupplyError::Malformed { .. }), "{error:?}");
    assert!(error.to_string().contains("results"), "{error}");
}
