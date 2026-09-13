//! TinyFish's search adapter, over the real transport, against its documented
//! response shape.
//!
//! `spec-verified` only: this operator holds no `TINYFISH_API_KEY`, so there is
//! no recorded capture and nothing to replay. Each test starts a loopback
//! origin serving a documented-shape response, points the adapter's base URL at
//! it, and asserts on both directions — the request the adapter put on the wire
//! and the envelopes and charge its parser derived from the reply. Nothing in
//! the path is a stand-in but the origin, which is the substitution `AGENTS.md`
//! permits.
//!
//! The loopback origin is the shared `tests/common` helper.

use commonmeasure_http::Response;
use commonmeasure_supply::{SupplyAdapter, SupplyError, TinyfishAdapter};
use commonmeasure_types::{ChargeBasis, LicenceState, Money};

mod common;

use common::Origin;

/// A documented-shape TinyFish response: two Guardian results, the first with a
/// snippet and a date, the second with neither.
fn documented_response() -> serde_json::Value {
    serde_json::json!({
        "query": "assisted dying bill vote",
        "results": [
            {
                "position": 1,
                "site_name": "theguardian.com",
                "title": "MPs debate assisted dying bill",
                "snippet": "Members of Parliament gathered to debate the assisted dying bill \
                             as it returned to the Commons.",
                "url": "https://www.theguardian.com/society/2026/aug/15/assisted-dying-bill",
                "date": "2026-08-15"
            },
            {
                "position": 2,
                "site_name": "theguardian.com",
                "title": "Reaction to the debate",
                "snippet": "",
                "url": "https://www.theguardian.com/society/2026/aug/10/reaction"
            }
        ],
        "total_results": 2,
        "page": 0
    })
}

#[test]
fn tinyfish_search_parses_its_documented_shape() {
    let response = documented_response();
    let origin =
        Origin::serving(move |_| Response::new(200, serde_json::to_vec(&response).unwrap()));

    let acquisition = TinyfishAdapter::new(&origin.url(), "tf-test-key")
        .search("assisted dying bill vote", 5, &[])
        .expect("documented shape");

    // Request: a GET with the query percent-encoded into the query string and
    // the key in the documented `X-API-Key` header. No body — TinyFish's
    // search is not a JSON POST.
    let request = origin.only_request();
    assert_eq!(request.method, "GET");
    assert_eq!(
        request.target, "/?query=assisted%20dying%20bill%20vote",
        "the job query must reach TinyFish's documented `query` parameter, encoded"
    );
    assert_eq!(request.header("X-API-Key"), Some("tf-test-key"));
    assert!(request.body.is_empty(), "a GET search carries no body");

    assert_eq!(acquisition.provider, "tinyfish");
    assert_eq!(acquisition.http_status, Some(200));
    // The documented response has no request-id field anywhere.
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
    // Accessibility is not permission — least of all for this supplier.
    assert_eq!(first.licence, LicenceState::Unknown);

    // `date` maps to the declared date, with provenance naming the field and
    // the provider.
    let dated = first
        .declared_date
        .as_ref()
        .expect("date maps to the declared date");
    assert_eq!(dated.date, "2026-08-15");
    assert!(
        dated.provenance.contains("supplier-declared") && dated.provenance.contains("TinyFish"),
        "the provenance must name who declared the date: {}",
        dated.provenance
    );

    // `position` and `site_name` are TinyFish's own fields and stay in native
    // metadata; promoted fields do not reappear under their own names.
    assert!(first.native_metadata.get("position").is_some());
    assert!(first.native_metadata.get("site_name").is_some());
    assert!(first.native_metadata.get("snippet").is_none());
    assert!(first.native_metadata.get("date").is_none());

    // The second result has an empty snippet and no `date`: it carries no
    // text, no content hash and no date rather than inventing them.
    let second = &acquisition.envelopes[1];
    assert!(second.text.is_none());
    assert!(second.content_hash.is_none());
    assert!(second.declared_date.is_none());

    // The response carries no cost field, and the published price of a search
    // is zero. A published zero is a price, not an absent cost: it is recorded
    // quoted, never observed.
    assert_eq!(
        acquisition.charge.money,
        Money::from_decimal_str("USD", "0")
    );
    let native = acquisition
        .charge
        .native
        .as_ref()
        .expect("the published free price is quoted, not left unknown");
    assert_eq!(native.unit, "USD");
    assert_eq!(native.amount.as_u64(), Some(0));
    assert_eq!(native.basis, ChargeBasis::Quoted);
}

/// TinyFish's `include_domains` filter is a comma-separated list, so the whole
/// allowed-host set is expressible: every named host must reach the request,
/// and when the job names none the parameter must be absent so an ordinary
/// open-web run is byte-identical to before.
#[test]
fn tinyfish_scopes_all_named_hosts_and_omits_the_parameter_otherwise() {
    let empty = || {
        Origin::serving(|_| {
            Response::new(
                200,
                serde_json::to_vec(
                    &serde_json::json!({"query": "q", "results": [], "total_results": 0, "page": 0}),
                )
                .unwrap(),
            )
        })
    };

    // Several hosts: all of them, comma-separated, in the documented form.
    let scoped = empty();
    TinyfishAdapter::new(&scoped.url(), "tf-test-key")
        .search("anything", 3, &["www.ft.com", "apnews.com"])
        .expect("documented shape");
    assert_eq!(
        scoped.only_request().target,
        "/?query=anything&include_domains=www.ft.com,apnews.com",
        "every named host must reach TinyFish's `include_domains`"
    );

    // No hosts: no `include_domains` at all.
    let unscoped = empty();
    TinyfishAdapter::new(&unscoped.url(), "tf-test-key")
        .search("anything", 3, &[])
        .expect("documented shape");
    assert_eq!(
        unscoped.only_request().target,
        "/?query=anything",
        "an unscoped search must not send `include_domains` at all"
    );
}

/// The request carries no result-count field, so the adapter caps the offered
/// candidates to the job's result limit itself; the sealed response still holds
/// everything TinyFish returned.
#[test]
fn tinyfish_caps_offered_results_to_the_limit_and_seals_the_rest() {
    let many = serde_json::json!({
        "query": "q",
        "results": (0..5).map(|i| serde_json::json!({
            "position": i + 1,
            "site_name": "ft.com",
            "title": "t",
            "snippet": "body",
            "url": format!("https://www.ft.com/content/{i}")
        })).collect::<Vec<_>>(),
        "total_results": 5,
        "page": 0
    });
    let origin = Origin::serving(move |_| Response::new(200, serde_json::to_vec(&many).unwrap()));

    let acquisition = TinyfishAdapter::new(&origin.url(), "tf-test-key")
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
fn tinyfish_treats_a_200_without_results_as_an_error_naming_the_field() {
    let origin = Origin::serving(|_| Response::new(200, b"{}".to_vec()));

    let error = TinyfishAdapter::new(&origin.url(), "tf-test-key")
        .search("anything", 1, &[])
        .expect_err("a body without `results` must fail");
    assert!(matches!(error, SupplyError::Malformed { .. }), "{error:?}");
    assert!(error.to_string().contains("results"), "{error}");
}
