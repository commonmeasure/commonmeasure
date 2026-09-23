//! Keenable's search adapter, over the real transport, against its documented
//! response shape.
//!
//! No recon capture of Keenable is committed, so there is nothing to replay
//! and these tests need no credential. Each test starts a loopback origin
//! serving a documented-shape response, points the adapter's base URL at it,
//! and asserts on both directions — the request the adapter put on the wire
//! and the envelopes and charge its parser derived from the reply. Nothing in
//! the path is a stand-in but the origin, which stands in for the network,
//! never for the code under test.
//!
//! The loopback origin is the shared `tests/common` helper.

use commonmeasure_http::Response;
use commonmeasure_supply::{KeenableAdapter, SupplyAdapter, SupplyError};
use commonmeasure_types::LicenceState;
use serde_json::Value;

mod common;

use common::Origin;

/// A documented-shape Keenable response: two Guardian results, the first with a
/// snippet and a publication date, the second without either.
fn documented_response() -> Value {
    serde_json::json!({
        "query": "assisted dying bill vote",
        "results": [
            {
                "url": "https://www.theguardian.com/society/2026/aug/15/assisted-dying-bill",
                "title": "MPs debate assisted dying bill",
                "description": "The bill returns to the Commons for its next reading.",
                "snippet": "Members of Parliament gathered to debate the assisted dying bill \
                             as it returned to the Commons.",
                "published_at": "2026-08-15T09:30:00Z",
                "acquired_at": "2026-08-16T02:00:00Z"
            },
            {
                "url": "https://www.theguardian.com/society/2026/aug/10/reaction",
                "title": "Reaction to the debate",
                "description": "Campaigners respond.",
                "snippet": "",
                "acquired_at": "2026-08-11T02:00:00Z"
            }
        ]
    })
}

#[test]
fn keenable_search_parses_its_documented_shape() {
    let response = documented_response();
    let origin =
        Origin::serving(move |_| Response::new(200, serde_json::to_vec(&response).unwrap()));

    let acquisition = KeenableAdapter::new(&origin.url(), "keen_test-key")
        .search("assisted dying bill vote", 5, &[])
        .expect("documented shape");

    // Request: the documented endpoint, method, auth header and query field.
    let request = origin.only_request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/v1/search");
    assert_eq!(request.header("X-API-Key"), Some("keen_test-key"));
    assert_eq!(
        request.json_body()["query"],
        "assisted dying bill vote",
        "the job query must reach Keenable's documented `query` field"
    );

    assert_eq!(acquisition.provider, "keenable");
    assert_eq!(acquisition.http_status, Some(200));
    // The search response has no request-id field.
    assert!(acquisition.provider_request_id.is_none());
    assert_eq!(acquisition.envelopes.len(), 2);

    let first = &acquisition.envelopes[0];
    assert_eq!(first.host, "www.theguardian.com");
    assert_eq!(
        first.source_url,
        "https://www.theguardian.com/society/2026/aug/15/assisted-dying-bill"
    );
    assert_eq!(
        first.title.as_deref(),
        Some("MPs debate assisted dying bill")
    );
    assert_eq!(first.retrieval_rank, 1);
    // `snippet` becomes the envelope text, and it is hashed.
    assert_eq!(
        first.text.as_deref(),
        Some(
            "Members of Parliament gathered to debate the assisted dying bill \
             as it returned to the Commons."
        )
    );
    assert!(
        first
            .content_hash
            .as_deref()
            .unwrap()
            .starts_with("sha256:"),
        "admitted text must be hashed at ingestion"
    );
    // Accessibility is not permission.
    assert_eq!(first.licence, LicenceState::Unknown);

    // `published_at` maps to the declared date, with provenance naming the
    // field and the provider.
    let dated = first
        .declared_date
        .as_ref()
        .expect("published_at maps to the declared date");
    assert_eq!(dated.date, "2026-08-15T09:30:00Z");
    assert!(
        dated.provenance.contains("supplier-declared") && dated.provenance.contains("Keenable"),
        "the provenance must name who declared the date: {}",
        dated.provenance
    );

    // `acquired_at` is Keenable's index date, not a publication date: it stays
    // in native metadata and never becomes the declared date. `description` is
    // a distinct field from the snippet and stays there too.
    assert!(first.native_metadata.get("acquired_at").is_some());
    assert!(first.native_metadata.get("description").is_some());
    // Promoted fields do not reappear under their own names.
    assert!(first.native_metadata.get("snippet").is_none());
    assert!(first.native_metadata.get("published_at").is_none());

    // The second result has an empty snippet and no `published_at`: it carries
    // no text, no content hash and no date rather than inventing them.
    let second = &acquisition.envelopes[1];
    assert!(second.text.is_none());
    assert!(second.content_hash.is_none());
    assert!(second.declared_date.is_none());

    // Keenable's search response carries no cost field and there is no published
    // flat price, so the charge is unknown — never recorded as zero.
    assert!(
        acquisition.charge.money.is_none() && acquisition.charge.native.is_none(),
        "an absent cost is unknown, not free"
    );
}

/// Keenable's `site` filter is single-valued: when the job names exactly one
/// allowed host, the adapter must scope the search to it; when it names none,
/// the field must be absent so an ordinary open-web run is byte-identical to
/// before; when it names several, the single-valued field cannot express the
/// set, so the search stays unscoped and admission enforces the host policy.
#[test]
fn keenable_scopes_to_a_single_host_and_omits_the_field_otherwise() {
    let empty = || {
        Origin::serving(|_| {
            Response::new(
                200,
                serde_json::to_vec(&serde_json::json!({"query": "q", "results": []})).unwrap(),
            )
        })
    };

    // Exactly one host: sent as `site`.
    let scoped = empty();
    KeenableAdapter::new(&scoped.url(), "keen_test-key")
        .search("anything", 3, &["www.ft.com"])
        .expect("documented shape");
    assert_eq!(
        scoped.only_request().json_body()["site"],
        serde_json::json!("www.ft.com"),
        "a single named host must reach Keenable as `site`"
    );

    // No hosts: no `site` at all.
    let unscoped = empty();
    KeenableAdapter::new(&unscoped.url(), "keen_test-key")
        .search("anything", 3, &[])
        .expect("documented shape");
    assert!(
        unscoped.only_request().json_body().get("site").is_none(),
        "an unscoped search must not send `site` at all"
    );

    // Several hosts: the single-valued field cannot carry them, so it is absent
    // and the search runs unscoped rather than dropping all but the first host.
    let multi = empty();
    KeenableAdapter::new(&multi.url(), "keen_test-key")
        .search("anything", 3, &["www.ft.com", "apnews.com"])
        .expect("documented shape");
    assert!(
        multi.only_request().json_body().get("site").is_none(),
        "several hosts cannot be expressed in the single-valued `site`, so it is omitted"
    );
}

/// The request carries no result-count field, so the adapter caps the offered
/// candidates to the job's result limit itself; the sealed response still holds
/// everything Keenable returned.
#[test]
fn keenable_caps_offered_results_to_the_limit_and_seals_the_rest() {
    let many = serde_json::json!({
        "query": "q",
        "results": (0..5).map(|i| serde_json::json!({
            "url": format!("https://www.ft.com/content/{i}"),
            "title": "t",
            "snippet": "body"
        })).collect::<Vec<_>>()
    });
    let origin = Origin::serving(move |_| Response::new(200, serde_json::to_vec(&many).unwrap()));

    let acquisition = KeenableAdapter::new(&origin.url(), "keen_test-key")
        .search("anything", 2, &[])
        .expect("documented shape");

    // Five returned, two requested: only two are offered, and the sealed
    // response still holds all five.
    assert_eq!(acquisition.envelopes.len(), 2);
    assert!(String::from_utf8_lossy(&acquisition.raw_response).contains("/content/4"));
}

/// A `200` whose body lacks `results` is a changed shape, not an empty
/// search; the error names the field and no acquisition (so no charge) exists.
#[test]
fn keenable_treats_a_200_without_results_as_an_error_naming_the_field() {
    let origin = Origin::serving(|_| Response::new(200, b"{}".to_vec()));

    let error = KeenableAdapter::new(&origin.url(), "keen_test-key")
        .search("anything", 1, &[])
        .expect_err("a body without `results` must fail");
    assert!(matches!(error, SupplyError::Malformed { .. }), "{error:?}");
    assert!(error.to_string().contains("results"), "{error}");
}
