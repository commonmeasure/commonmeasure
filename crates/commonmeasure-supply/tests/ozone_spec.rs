//! Ozone Live's adapter, over the real transport, against its documented
//! response shapes.
//!
//! No recon capture of Ozone is committed, so there is nothing to replay and
//! these tests need no credential. Each test starts a loopback origin serving
//! a documented-shape response (`https://api.ozone.live/openapi.yaml`), points
//! the adapter's base URL at it, and asserts on both directions — the request
//! the adapter put on the wire and the envelopes and charge its parser derived
//! from the reply. Nothing in the path is a stand-in but the origin, which
//! stands in for the network, never for the code under test.
//!
//! The loopback origin is the shared `tests/common` helper.

use commonmeasure_http::Response;
use commonmeasure_supply::{OzoneAdapter, SupplyAdapter, SupplyError};
use commonmeasure_types::{LicenceState, ProviderCapability};
use serde_json::{Value, json};

mod common;

use common::Origin;

/// A documented-shape search response grouped by article: two rows, the first
/// with a passage, a date, publisher identity and a nested second chunk, the
/// second with no text, no date and no publisher resolved.
fn documented_search_response() -> Value {
    json!({
        "query": "local election candidates on transport",
        "provider": "ozone",
        "group_by": "article",
        "results": [
            {
                "id": "2d735536f4f58f43_25",
                "score": 0.5933,
                "url": "https://www.coventrytelegraph.net/news/coventry-news/general-election-2019-quiz-candidates-17345678",
                "domain": "coventrytelegraph.net",
                "title": "General Election 2019: We quiz the candidates",
                "text": "The candidates were asked how they would improve bus services across the city.",
                "summary": "Candidates answer questions on transport.",
                "section_heading": "About national issues (continued)",
                "published_date": "2026-04-29",
                "article_id": "2d735536f4f58f43",
                "chunk_index": 25,
                "publisher_id": "BIDDEROPX001",
                "publisher_name": "Reach plc",
                "article_type": "hard_news",
                "temporal_sensitivity": "historical",
                "language": "en",
                "licensed": true,
                "additional_chunks": [
                    {
                        "id": "2d735536f4f58f43_26",
                        "score": 0.51,
                        "text": "A second passage from the same article.",
                        "chunk_index": 26
                    }
                ]
            },
            {
                "id": "9a1b2c3d4e5f6a7b_3",
                "score": 0.41,
                "url": "https://www.bbc.com/news/articles/c1234567890",
                "domain": "bbc.com",
                "title": "",
                "text": "",
                "summary": "",
                "section_heading": "",
                "published_date": "",
                "article_id": "9a1b2c3d4e5f6a7b",
                "chunk_index": 3,
                "publisher_id": "OZONEBBC4784",
                "publisher_name": "",
                "article_type": "hard_news",
                "temporal_sensitivity": "current",
                "language": "en",
                "licensed": true
            }
        ],
        "usage": {
            "embedding_tokens": 9,
            "embedding_cached": false,
            "latency_ms": 212
        }
    })
}

#[test]
fn ozone_search_parses_its_documented_shape() {
    let response = documented_search_response();
    let origin =
        Origin::serving(move |_| Response::new(200, serde_json::to_vec(&response).unwrap()));

    let acquisition = OzoneAdapter::new(&origin.url(), "oz_test-key")
        .search("local election candidates on transport", 5, &[])
        .expect("documented shape");

    // Request: the documented endpoint, method, bearer header, count and
    // grouping. No filters when the job named no hosts.
    let request = origin.only_request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/v1/search");
    assert_eq!(request.header("Authorization"), Some("Bearer oz_test-key"));
    let body = request.json_body();
    assert_eq!(
        body["query"], "local election candidates on transport",
        "the job query must reach Ozone's documented `query` field"
    );
    assert_eq!(body["top_k"], 5, "the job limit must reach `top_k`");
    assert_eq!(
        body["group_by"], "article",
        "one row per document keeps a result page-granular like every other provider"
    );
    assert_eq!(body["include_text"], true);
    assert!(
        body.get("filters").is_none(),
        "an unscoped search must not send `filters` at all"
    );
    assert!(
        body.get("include_answer").is_none(),
        "Ozone is retrieval only and rejects answer generation; nothing may ask for it"
    );

    assert_eq!(acquisition.provider, "ozone");
    assert_eq!(acquisition.capability, ProviderCapability::Search);
    assert_eq!(acquisition.http_status, Some(200));
    // The documented response has no request-id field.
    assert!(acquisition.provider_request_id.is_none());
    assert_eq!(acquisition.envelopes.len(), 2);

    let first = &acquisition.envelopes[0];
    assert_eq!(first.host, "www.coventrytelegraph.net");
    assert_eq!(
        first.title.as_deref(),
        Some("General Election 2019: We quiz the candidates")
    );
    assert_eq!(first.retrieval_rank, 1);
    // The row's own passage becomes the envelope text, and it is hashed. The
    // nested chunk is not folded in.
    assert_eq!(
        first.text.as_deref(),
        Some("The candidates were asked how they would improve bus services across the city.")
    );
    assert!(
        first
            .content_hash
            .as_deref()
            .unwrap()
            .starts_with("sha256:"),
        "admitted text must be hashed at ingestion"
    );
    // `licensed: true` is a boolean about Ozone's corpus, not a licence
    // reference the operator can name in a rule: the envelope stays unknown
    // and the boolean and publisher identity stay in native metadata.
    assert_eq!(first.licence, LicenceState::Unknown);
    assert_eq!(first.native_metadata["licensed"], true);
    assert_eq!(first.native_metadata["publisher_id"], "BIDDEROPX001");
    assert_eq!(first.native_metadata["publisher_name"], "Reach plc");
    assert!(
        first.native_metadata.get("additional_chunks").is_some(),
        "nested passages stay in native metadata"
    );
    assert!(first.native_metadata.get("summary").is_some());
    assert!(first.native_metadata.get("score").is_some());
    // Promoted fields do not reappear under their own names.
    assert!(first.native_metadata.get("text").is_none());
    assert!(first.native_metadata.get("title").is_none());
    assert!(first.native_metadata.get("published_date").is_none());

    // `published_date` maps to the declared date, with provenance naming the
    // field and the provider.
    let dated = first
        .declared_date
        .as_ref()
        .expect("published_date maps to the declared date");
    assert_eq!(dated.date, "2026-04-29");
    assert!(
        dated.provenance.contains("supplier-declared") && dated.provenance.contains("Ozone"),
        "the provenance must name who declared the date: {}",
        dated.provenance
    );

    // The second row has empty text, title, date and publisher name: it
    // carries no text, no content hash, no title and no date rather than
    // inventing them. Ozone's rule is that an absent value is empty, never
    // fabricated, and the adapter keeps it that way.
    let second = &acquisition.envelopes[1];
    assert_eq!(second.host, "www.bbc.com");
    assert!(second.text.is_none());
    assert!(second.content_hash.is_none());
    assert!(second.title.is_none());
    assert!(second.declared_date.is_none());
    assert_eq!(second.retrieval_rank, 2);

    // No cost field and no published price: the charge is unknown, never zero.
    // `usage` is consumption, not a charge.
    assert!(
        acquisition.charge.money.is_none() && acquisition.charge.native.is_none(),
        "an absent cost is unknown, not free"
    );
    assert!(String::from_utf8_lossy(&acquisition.raw_response).contains("embedding_tokens"));
}

/// The job's allowed hosts travel as `filters.domains` with `mode: include`,
/// all of them in one request; absent when the job named none (asserted
/// above).
#[test]
fn ozone_scopes_to_every_allowed_host_in_one_filter() {
    let origin = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&json!({"query": "q", "provider": "ozone", "results": []})).unwrap(),
        )
    });

    let acquisition = OzoneAdapter::new(&origin.url(), "oz_test-key")
        .search("anything", 3, &["www.bbc.com", "www.theguardian.com"])
        .expect("documented shape");

    let body = origin.only_request().json_body();
    assert_eq!(
        body["filters"]["domains"],
        json!({"values": ["www.bbc.com", "www.theguardian.com"], "mode": "include"}),
        "every allowed host must reach Ozone's `filters.domains` include list"
    );
    assert!(
        body["filters"].get("recency").is_none(),
        "no date window is sent: freshness is scoped after admission, not at the supplier"
    );
    // A present, empty `results` is a legitimate empty search.
    assert!(acquisition.envelopes.is_empty());
}

/// `top_k` is documented 1–100. A job asking for more gets the ceiling, and
/// the adapter declares that ceiling so the runtime records the shortfall.
#[test]
fn ozone_clamps_top_k_to_its_documented_ceiling() {
    let origin = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&json!({"query": "q", "provider": "ozone", "results": []})).unwrap(),
        )
    });

    let adapter = OzoneAdapter::new(&origin.url(), "oz_test-key");
    assert_eq!(adapter.maximum_search_results(), Some(100));
    adapter
        .search("anything", 250, &[])
        .expect("documented shape");
    assert_eq!(origin.only_request().json_body()["top_k"], 100);
}

/// A documented-shape `/v1/contents` response: one corpus article served from
/// licensed storage, one live-fetched page marked unlicensed, one failure.
fn documented_contents_response() -> Value {
    json!({
        "provider": "ozone",
        "count": 3,
        "succeeded": 2,
        "results": [
            {
                "ok": true,
                "article_id": "2d735536f4f58f43",
                "url": "https://www.coventrytelegraph.net/news/coventry-news/general-election-2019-quiz-candidates-17345678",
                "source": "gcs_markdown",
                "text": "# General Election 2019\n\nThe full article body.",
                "chars": 48,
                "licensed": true
            },
            {
                "ok": true,
                "url": "https://example.org/not-in-corpus",
                "source": "live_fetch",
                "text": "A page Ozone fetched live.",
                "chars": 26,
                "licensed": false
            },
            {
                "ok": false,
                "url": "https://example.org/blocked",
                "error": {"code": "ssrf_blocked", "message": "target resolves to a private address"}
            }
        ]
    })
}

#[test]
fn ozone_fetch_parses_its_documented_shape() {
    let response = documented_contents_response();
    let origin =
        Origin::serving(move |_| Response::new(200, serde_json::to_vec(&response).unwrap()));

    let acquisition = OzoneAdapter::new(&origin.url(), "oz_test-key")
        .fetch("https://www.coventrytelegraph.net/news/coventry-news/general-election-2019-quiz-candidates-17345678")
        .expect("documented shape");

    let request = origin.only_request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/v1/contents");
    assert_eq!(request.header("Authorization"), Some("Bearer oz_test-key"));
    assert_eq!(
        request.json_body()["urls"],
        json!([
            "https://www.coventrytelegraph.net/news/coventry-news/general-election-2019-quiz-candidates-17345678"
        ]),
        "the named URL must reach Ozone's documented `urls` list"
    );

    assert_eq!(acquisition.capability, ProviderCapability::Fetch);
    // Two served items become envelopes; the failed item yields none and
    // stays in the sealed response with Ozone's own error.
    assert_eq!(acquisition.envelopes.len(), 2);
    assert!(String::from_utf8_lossy(&acquisition.raw_response).contains("ssrf_blocked"));

    let corpus = &acquisition.envelopes[0];
    assert_eq!(corpus.host, "www.coventrytelegraph.net");
    assert_eq!(
        corpus.text.as_deref(),
        Some("# General Election 2019\n\nThe full article body.")
    );
    assert!(corpus.content_hash.is_some());
    assert!(corpus.title.is_none(), "ContentItem documents no title");
    assert!(
        corpus.declared_date.is_none(),
        "ContentItem documents no date"
    );
    assert_eq!(corpus.licence, LicenceState::Unknown);
    assert_eq!(corpus.native_metadata["source"], "gcs_markdown");
    assert_eq!(corpus.native_metadata["licensed"], true);

    // A live-fetched page is an ordinary third-party fetch and says so.
    let live = &acquisition.envelopes[1];
    assert_eq!(live.host, "example.org");
    assert_eq!(live.licence, LicenceState::Unknown);
    assert_eq!(live.native_metadata["source"], "live_fetch");
    assert_eq!(live.native_metadata["licensed"], false);

    assert!(
        acquisition.charge.money.is_none() && acquisition.charge.native.is_none(),
        "no cost field on the contents response: unknown, not free"
    );
}

/// A `200` whose body lacks `results` is a changed shape, not an empty
/// search; the error names the field and no acquisition (so no charge) exists.
#[test]
fn ozone_treats_a_200_without_results_as_an_error_naming_the_field() {
    let origin = Origin::serving(|_| Response::new(200, b"{\"provider\":\"ozone\"}".to_vec()));

    let error = OzoneAdapter::new(&origin.url(), "oz_test-key")
        .search("anything", 1, &[])
        .expect_err("a body without `results` must fail");
    assert!(matches!(error, SupplyError::Malformed { .. }), "{error:?}");
    assert!(error.to_string().contains("results"), "{error}");
}

/// Ozone's documented error envelope, `{"error":{"code","message"}}`, arrives
/// on a non-2xx status and surfaces with the provider's own words, so a
/// reviewer can read the stable code from the record.
#[test]
fn ozone_surfaces_its_documented_error_code() {
    let origin = Origin::serving(|_| {
        Response::new(
            429,
            serde_json::to_vec(&json!({
                "error": {"code": "upstream_rate_limited", "message": "slow down"}
            }))
            .unwrap(),
        )
    });

    let error = OzoneAdapter::new(&origin.url(), "oz_test-key")
        .search("anything", 1, &[])
        .expect_err("a 429 is the provider refusing");
    match error {
        SupplyError::Status { status, detail, .. } => {
            assert_eq!(status, 429);
            assert!(detail.contains("upstream_rate_limited"), "{detail}");
        }
        other => panic!("expected a status error, got {other:?}"),
    }
}
