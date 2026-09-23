//! The Nimble adapter, over the real transport, against Nimble's documented
//! response shape served from a loopback origin.
//!
//! No recon capture of Nimble is committed, so these tests exercise the
//! parser over the documented shapes rather than recorded bytes, and need no
//! credential. Nothing in the path is a stand-in — the client, the framing,
//! the parser and the charge rules are the ones a live call would use. Only
//! the origin is local: it stands in for the network, never for the code
//! under test.
//!
//! This is a separate test crate, so the small loopback-origin helper is copied
//! from `recorded_replay.rs` rather than shared — each `tests/*.rs` compiles on
//! its own and there is no shared symbol to import.

use commonmeasure_http::Response;
use commonmeasure_supply::{NimbleAdapter, SupplyAdapter, SupplyError};
use commonmeasure_types::{ChargeBasis, LicenceState};
use serde_json::Value;

mod common;

use common::Origin;

/// A documented-shape Nimble search response: `request_id`, `total_results`, a
/// `results` array of `{title, description, url, content, metadata}`. One target
/// publisher (theguardian.com) is used for realism, and the second result
/// carries an empty `content` to prove a textless candidate stays textless.
fn documented_response() -> Value {
    serde_json::json!({
        "request_id": "b6b3f0e2-0c2a-4d1e-9f7a-2a4c8e1d5b90",
        "total_results": 2,
        "results": [
            {
                "title": "Guardian investigation into AI scraping",
                "description": "A short SERP snippet describing the page.",
                "url": "https://www.theguardian.com/technology/2026/aug/01/ai-scraping",
                "content": "The full extracted body of the article, as Nimble returns it at fast depth.",
                "metadata": {
                    "position": 1,
                    "entity_type": "organic",
                    "country": "GB",
                    "locale": "en"
                }
            },
            {
                "title": "A second result with no extracted content",
                "description": "Another SERP snippet.",
                "url": "https://www.theguardian.com/technology/2026/aug/02/follow-up",
                "content": "",
                "metadata": {"position": 2}
            }
        ]
    })
}

#[test]
fn nimble_search_parses_its_documented_shape() {
    let origin = Origin::serving(|_| {
        Response::new(200, serde_json::to_vec(&documented_response()).unwrap())
    });

    let acquisition = NimbleAdapter::new(&origin.url(), "test-key")
        .search("guardian AI scraping investigation", 5, &[])
        .expect("documented shape should parse");

    // Request assertions: endpoint path, auth header, and the query reaching
    // the documented field.
    let request = origin.only_request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/v2/search");
    assert_eq!(request.header("Authorization"), Some("Bearer test-key"));
    assert_eq!(
        request.json_body()["query"],
        "guardian AI scraping investigation"
    );
    assert_eq!(request.json_body()["max_results"], 5);
    assert_eq!(request.json_body()["search_depth"], "fast");

    // Response parse.
    assert_eq!(acquisition.provider, "nimble");
    assert_eq!(acquisition.http_status, Some(200));
    assert_eq!(
        acquisition.provider_request_id.as_deref(),
        Some("b6b3f0e2-0c2a-4d1e-9f7a-2a4c8e1d5b90")
    );
    assert_eq!(acquisition.envelopes.len(), 2);

    let first = &acquisition.envelopes[0];
    assert_eq!(first.host, "www.theguardian.com");
    assert_eq!(
        first.source_url,
        "https://www.theguardian.com/technology/2026/aug/01/ai-scraping"
    );
    assert_eq!(
        first.title.as_deref(),
        Some("Guardian investigation into AI scraping")
    );
    assert_eq!(first.retrieval_rank, 1);
    assert_eq!(
        first.text.as_deref(),
        Some("The full extracted body of the article, as Nimble returns it at fast depth.")
    );
    assert!(
        first
            .content_hash
            .as_deref()
            .unwrap()
            .starts_with("sha256:"),
        "admitted text must be hashed at ingestion"
    );
    // No Nimble search response carries a licence field. Search accessibility is
    // not permission, so the envelope says unknown rather than infer one.
    assert_eq!(first.licence, LicenceState::Unknown);
    // Nimble documents no per-result date, so none may be mapped.
    for envelope in &acquisition.envelopes {
        assert!(
            envelope.declared_date.is_none(),
            "Nimble documents no per-result date, so none may be mapped"
        );
    }
    // The SERP snippet and metadata stay namespaced rather than promoted, so a
    // fact does not appear twice under two names.
    assert!(first.native_metadata.get("description").is_some());
    assert!(first.native_metadata.get("metadata").is_some());
    assert!(first.native_metadata.get("content").is_none());
    assert!(first.native_metadata.get("title").is_none());

    // A result whose `content` is empty falls back to the SERP snippet rather
    // than being reported textless; the snippet is then promoted out of native
    // metadata so it does not appear twice.
    let second = &acquisition.envelopes[1];
    assert_eq!(second.text.as_deref(), Some("Another SERP snippet."));
    assert!(
        second
            .content_hash
            .as_deref()
            .unwrap()
            .starts_with("sha256:")
    );
    assert!(second.native_metadata.get("description").is_none());

    // Nimble discloses no cost field on the search response, so the charge is
    // the published fast-depth price, marked quoted — never observed, never zero.
    assert!(
        acquisition.charge.money.is_none(),
        "Nimble reports no currency and must not be given one"
    );
    let native = acquisition
        .charge
        .native
        .expect("a quoted charge is recorded");
    assert_eq!(native.unit, "credits");
    assert_eq!(native.amount.as_u64(), Some(2));
    assert_eq!(native.basis, ChargeBasis::Quoted);
    assert!(native.note.is_some(), "a quoted charge must say why");
}

/// A job's allowed source hosts scope the Nimble search through
/// `include_domains`: when the operator names hosts, the adapter asks Nimble for
/// only those, so the run does not pay to fetch sources admission would reject.
/// An unscoped search must send no such field, so an ordinary open-web run is
/// byte-identical to before.
#[test]
fn nimble_scopes_the_search_to_the_jobs_allowed_hosts_and_omits_the_field_when_there_are_none() {
    let scoped = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({"results": []})).unwrap(),
        )
    });
    NimbleAdapter::new(&scoped.url(), "test-key")
        .search("anything", 2, &["www.ft.com", "apnews.com"])
        .expect("documented shape");
    assert_eq!(
        scoped.only_request().json_body()["include_domains"],
        serde_json::json!(["www.ft.com", "apnews.com"]),
        "named hosts must reach Nimble as include_domains"
    );

    let unscoped = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({"results": []})).unwrap(),
        )
    });
    NimbleAdapter::new(&unscoped.url(), "test-key")
        .search("anything", 2, &[])
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

/// A body the parser cannot read is an error, not an empty result: a broken
/// parser must not read like a quiet provider that found nothing.
#[test]
fn nimble_treats_an_unreadable_body_as_an_error() {
    let origin = Origin::serving(|_| Response::new(200, b"<html>not json</html>".to_vec()));

    let error = NimbleAdapter::new(&origin.url(), "test-key")
        .search("anything", 1, &[])
        .expect_err("an unparseable body must fail");
    assert!(matches!(error, SupplyError::Malformed { .. }), "{error:?}");
}

/// A `200` whose body lacks `results` is a changed shape, not an empty
/// search; the error names the field and no acquisition (so no charge) exists.
#[test]
fn nimble_treats_a_200_without_results_as_an_error_naming_the_field() {
    let origin = Origin::serving(|_| Response::new(200, b"{}".to_vec()));

    let error = NimbleAdapter::new(&origin.url(), "test-key")
        .search("anything", 1, &[])
        .expect_err("a body without `results` must fail");
    assert!(matches!(error, SupplyError::Malformed { .. }), "{error:?}");
    assert!(error.to_string().contains("results"), "{error}");
}
