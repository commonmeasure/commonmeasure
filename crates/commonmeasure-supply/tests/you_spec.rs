//! The You.com adapter, over the real transport, against You.com's documented
//! response shape served by a loopback origin.
//!
//! No recon capture of You.com is committed, so these tests exercise the
//! parser over the documented shapes rather than recorded bytes, and need no
//! credential. Nothing in the path is a stand-in — the client, the framing,
//! the parser and the charge rules are the ones a live call would use. Only
//! the origin is local: it stands in for the network, never for the code
//! under test.
//!
//! Each `tests/*.rs` is its own test crate with no shared symbols, so the small
//! loopback-origin helper here is deliberately a local copy of the one in
//! `recorded_replay.rs`.

use commonmeasure_http::Response;
use commonmeasure_supply::{SupplyAdapter, SupplyError, YouAdapter};
use commonmeasure_types::{ChargeBasis, LicenceState};
use serde_json::Value;

mod common;

use common::Origin;

/// A documented-shape You.com response: two `results.web` items on a target
/// publisher that blocks AI scraping, one carrying `snippets` and `page_age`
/// and one carrying neither, plus a `metadata.search_uuid`.
fn documented_body() -> Value {
    serde_json::json!({
        "results": {
            "web": [
                {
                    "url": "https://www.theguardian.com/world/2026/aug/15/example",
                    "title": "A Guardian headline",
                    "description": "A one-line summary of the article.",
                    "snippets": ["First fragment of the page.", "Second fragment."],
                    "page_age": "2026-08-15T09:00:00Z",
                    "favicon_url": "https://www.theguardian.com/favicon.ico"
                },
                {
                    "url": "https://www.theguardian.com/world/2026/aug/14/other",
                    "title": "Another headline",
                    "description": "Another summary.",
                    "snippets": []
                }
            ],
            "news": []
        },
        "metadata": {
            "query": "royal family news",
            "search_uuid": "8a911eb2-7c7a-4afa-a20d-0d9dc98d07c0",
            "latency": 0.42
        }
    })
}

#[test]
fn you_search_parses_its_documented_shape() {
    let body = documented_body();
    let origin = Origin::serving(move |_| Response::new(200, serde_json::to_vec(&body).unwrap()));

    let acquisition = YouAdapter::new(&origin.url(), "test-key")
        .search("royal family news", 5, &[])
        .expect("documented shape");

    // The request: POST to the documented path, X-API-Key auth, the query and
    // the result count in the documented body fields.
    let request = origin.only_request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/v1/search");
    assert_eq!(request.header("X-API-Key"), Some("test-key"));
    assert_eq!(request.json_body()["query"], "royal family news");
    assert_eq!(request.json_body()["count"], 5);

    assert_eq!(acquisition.provider, "you");
    assert_eq!(acquisition.http_status, Some(200));
    assert_eq!(
        acquisition.provider_request_id.as_deref(),
        Some("8a911eb2-7c7a-4afa-a20d-0d9dc98d07c0")
    );
    assert_eq!(acquisition.envelopes.len(), 2);

    let first = &acquisition.envelopes[0];
    assert_eq!(
        first.source_url,
        "https://www.theguardian.com/world/2026/aug/15/example"
    );
    assert_eq!(first.host, "www.theguardian.com");
    assert_eq!(first.title.as_deref(), Some("A Guardian headline"));
    assert_eq!(first.retrieval_rank, 1);
    // The snippets array is joined into the one text field, and hashed.
    assert_eq!(
        first.text.as_deref(),
        Some("First fragment of the page.\n\nSecond fragment.")
    );
    assert!(
        first
            .content_hash
            .as_deref()
            .unwrap()
            .starts_with("sha256:"),
        "admitted text must be hashed at ingestion"
    );
    // No You.com response carries a licence field. Search accessibility is not
    // permission, so the envelope must say unknown rather than infer one.
    assert_eq!(first.licence, LicenceState::Unknown);
    // `page_age` maps to the declared date with provenance naming the field.
    let dated = first
        .declared_date
        .as_ref()
        .expect("page_age maps to a date");
    assert_eq!(dated.date, "2026-08-15T09:00:00Z");
    assert!(
        dated.provenance.contains("supplier-declared") && dated.provenance.contains("You.com"),
        "the provenance must name who declared the date: {}",
        dated.provenance
    );
    // The summary stays namespaced; promoted fields do not reappear by name.
    assert!(first.native_metadata.get("description").is_some());
    assert!(first.native_metadata.get("favicon_url").is_some());
    assert!(first.native_metadata.get("snippets").is_none());
    assert!(first.native_metadata.get("page_age").is_none());

    // A result with an empty snippets array carries no text and no date.
    let second = &acquisition.envelopes[1];
    assert!(second.text.is_none());
    assert!(second.content_hash.is_none());
    assert!(second.declared_date.is_none());

    // You.com reports no charge on the response, so the only honest figure is
    // the published price, marked quoted. It is stated in currency but was not
    // reported on this response, so `money` stays absent and the price rides
    // the native charge alone.
    assert!(
        acquisition.charge.money.is_none(),
        "a quoted price is not a currency charge reported on the response"
    );
    let native = acquisition
        .charge
        .native
        .expect("a quoted charge is recorded");
    assert_eq!(native.unit, "USD");
    assert_eq!(native.basis, ChargeBasis::Quoted);
    assert!(native.note.is_some(), "a quoted charge must say why");
}

/// A job's allowed source hosts scope the You.com search through
/// `include_domains`; with none named the field is absent entirely, so an
/// ordinary open-web run is byte-identical to before.
#[test]
fn you_scopes_the_search_to_the_jobs_allowed_hosts_and_omits_the_field_when_there_are_none() {
    let empty = serde_json::json!({"results": {"web": []}, "metadata": {}});

    let scoped = Origin::serving({
        let empty = empty.clone();
        move |_| Response::new(200, serde_json::to_vec(&empty).unwrap())
    });
    YouAdapter::new(&scoped.url(), "test-key")
        .search("anything", 3, &["theguardian.com", "ft.com"])
        .expect("documented shape");
    assert_eq!(
        scoped.only_request().json_body()["include_domains"],
        serde_json::json!(["theguardian.com", "ft.com"]),
        "named hosts must reach You.com as include_domains"
    );

    let unscoped =
        Origin::serving(move |_| Response::new(200, serde_json::to_vec(&empty).unwrap()));
    YouAdapter::new(&unscoped.url(), "test-key")
        .search("anything", 3, &[])
        .expect("documented shape");
    assert!(
        unscoped
            .only_request()
            .json_body()
            .get("include_domains")
            .is_none(),
        "an unscoped search must not send include_domains at all"
    );
}

/// A `200` whose body lacks `results.web` is a changed shape, not an
/// empty search; the error names the field and no acquisition (so no charge)
/// exists.
#[test]
fn you_treats_a_200_without_results_as_an_error_naming_the_field() {
    let origin = Origin::serving(|_| Response::new(200, b"{}".to_vec()));

    let error = YouAdapter::new(&origin.url(), "test-key")
        .search("anything", 1, &[])
        .expect_err("a body without `results.web` must fail");
    assert!(matches!(error, SupplyError::Malformed { .. }), "{error:?}");
    assert!(error.to_string().contains("results"), "{error}");
}
