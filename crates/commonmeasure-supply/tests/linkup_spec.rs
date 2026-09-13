//! Linkup's verification state and the artefacts behind it are recorded in
//! `docs/knowledge-base/provider-verification.md`; no recon capture is
//! committed, so these tests exercise the parser over Linkup's documented
//! `searchResults` response shape through the real transport and a loopback
//! origin, and need no credential. Nothing in the path is a stand-in — the client, the
//! framing, the parser and the charge rules are the ones a live call would use.
//! Only the origin is local, which is the substitution `AGENTS.md` permits.
//!
//! Each `tests/*.rs` is its own test crate, so the small loopback-origin helper
//! is copied from the `recorded_replay.rs` pattern rather than shared.

use commonmeasure_http::Response;
use commonmeasure_supply::{LinkupAdapter, SupplyAdapter, SupplyError};
use commonmeasure_types::{ChargeBasis, LicenceState, ProviderCapability};
mod common;

use common::Origin;

/// The request must hit `POST /v1/search` with `Authorization: Bearer`, carry
/// the query in `q`, ask for `searchResults` at `standard` depth with
/// `maxResults` set to the job limit, and the documented `results` array of
/// `{type, name, url, content, favicon}` must become the envelopes: `name` the
/// title, `content` the hashed text, licence unknown, no date. Linkup discloses
/// no cost on the response, so the charge is the published $0.005 marked quoted.
#[test]
fn linkup_search_parses_its_documented_shape() {
    let origin = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({
                "results": [
                    {
                        "type": "text",
                        "name": "Guardian report on the story",
                        "url": "https://www.theguardian.com/world/2026/aug/15/story",
                        "content": "The opening paragraph of the page as Linkup returned it.",
                        "favicon": "https://www.theguardian.com/favicon.ico"
                    },
                    {
                        "type": "text",
                        "name": "A second source",
                        "url": "https://www.ft.com/content/second",
                        "content": "",
                        "favicon": ""
                    }
                ]
            }))
            .unwrap(),
        )
    });

    let acquisition = LinkupAdapter::new(&origin.url(), "test-key")
        .search("what happened in the story", 5, &[])
        .expect("documented shape");

    let request = origin.only_request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/v1/search");
    assert_eq!(request.header("Authorization"), Some("Bearer test-key"));
    assert_eq!(request.json_body()["q"], "what happened in the story");
    assert_eq!(request.json_body()["depth"], "standard");
    assert_eq!(request.json_body()["outputType"], "searchResults");
    assert_eq!(request.json_body()["maxResults"], 5);

    assert_eq!(acquisition.provider, "linkup");
    assert_eq!(acquisition.http_status, Some(200));
    // No request identifier is documented on the searchResults response.
    assert!(acquisition.provider_request_id.is_none());
    assert_eq!(acquisition.envelopes.len(), 2);

    let first = &acquisition.envelopes[0];
    assert_eq!(first.host, "www.theguardian.com");
    assert_eq!(
        first.source_url,
        "https://www.theguardian.com/world/2026/aug/15/story"
    );
    assert_eq!(first.retrieval_rank, 1);
    assert_eq!(first.title.as_deref(), Some("Guardian report on the story"));
    assert_eq!(
        first.text.as_deref(),
        Some("The opening paragraph of the page as Linkup returned it.")
    );
    assert!(
        first
            .content_hash
            .as_deref()
            .unwrap()
            .starts_with("sha256:"),
        "admitted text must be hashed at ingestion"
    );
    // No Linkup response carries a licence field; accessibility is not
    // permission, so the envelope must say unknown rather than infer one.
    assert_eq!(first.licence, LicenceState::Unknown);
    // Linkup returns no per-result date, so none may be mapped.
    for envelope in &acquisition.envelopes {
        assert!(
            envelope.declared_date.is_none(),
            "Linkup declares no date, so none may be mapped"
        );
    }
    // Provider-specific fields stay namespaced rather than promoted.
    assert!(first.native_metadata.get("favicon").is_some());
    assert!(first.native_metadata.get("type").is_some());
    assert!(first.native_metadata.get("content").is_none());
    assert!(first.native_metadata.get("name").is_none());

    // A result with an empty content string carries no text and no hash.
    assert!(acquisition.envelopes[1].text.is_none());
    assert!(acquisition.envelopes[1].content_hash.is_none());

    // Linkup reports no cost on the response, so the charge is the published
    // price, marked quoted, disclosed in currency.
    let money = acquisition.charge.money.expect("Linkup quotes a USD price");
    assert_eq!(money.currency(), "USD");
    assert_eq!(money.micros, 5_000);
    let native = acquisition
        .charge
        .native
        .expect("a quoted charge is recorded");
    assert_eq!(native.unit, "USD");
    assert_eq!(native.basis, ChargeBasis::Quoted);
    assert_eq!(native.amount.to_string(), "0.005");
    assert!(native.note.is_some(), "a quoted charge must say why");
}

/// A job's allowed hosts scope the Linkup search through `includeDomains`; with
/// none named the field is absent entirely, so an ordinary open-web run is
/// byte-identical to before.
#[test]
fn linkup_scopes_the_search_to_the_jobs_allowed_hosts_and_omits_the_field_when_there_are_none() {
    let scoped = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({"results": []})).unwrap(),
        )
    });
    LinkupAdapter::new(&scoped.url(), "test-key")
        .search("anything", 2, &["theguardian.com", "ft.com"])
        .expect("documented shape");
    assert_eq!(
        scoped.only_request().json_body()["includeDomains"],
        serde_json::json!(["theguardian.com", "ft.com"]),
        "named hosts must reach Linkup as includeDomains"
    );

    let unscoped = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({"results": []})).unwrap(),
        )
    });
    LinkupAdapter::new(&unscoped.url(), "test-key")
        .search("anything", 2, &[])
        .expect("documented shape");
    assert!(
        unscoped
            .only_request()
            .json_body()
            .get("includeDomains")
            .is_none(),
        "an unscoped search must not send includeDomains at all"
    );
}

/// A result whose URL has no readable host cannot be checked against a source
/// policy, so it is not offered as supply; the sealed response still holds it.
#[test]
fn linkup_drops_a_result_with_no_readable_host_but_seals_it() {
    let origin = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({
                "results": [
                    {"type": "text", "name": "t", "url": "not a url at all", "content": "body"},
                    {"type": "text", "name": "t", "url": "https://apnews.com/article/x", "content": "body"}
                ]
            }))
            .unwrap(),
        )
    });

    let acquisition = LinkupAdapter::new(&origin.url(), "test-key")
        .search("anything", 5, &[])
        .expect("documented shape");
    let hosts: Vec<&str> = acquisition
        .envelopes
        .iter()
        .map(|envelope| envelope.host.as_str())
        .collect();
    assert_eq!(hosts, ["apnews.com"]);
    assert!(
        String::from_utf8_lossy(&acquisition.raw_response).contains("not a url at all"),
        "the provider's own answer is still sealed whole"
    );
}

/// Linkup's `fetch` capability is `/v1/fetch`: it names one URL and returns the
/// page body as `markdown`. The response carries no URL of its own, so the
/// source is the requested URL, and no title is invented. It carries no cost
/// field and no per-fetch price has been read, so the charge is unknown —
/// nothing observed, nothing quoted, never zero.
#[test]
fn linkup_fetch_returns_page_markdown() {
    let origin = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({
                "favicon": "https://favicons.linkup.so?domain=www.ft.com",
                "markdown": "The full article markdown body."
            }))
            .unwrap(),
        )
    });
    let acquisition = LinkupAdapter::new(&origin.url(), "test-key")
        .fetch("https://www.ft.com/content/x")
        .expect("documented shape");
    let request = origin.only_request();
    assert_eq!(request.target, "/v1/fetch");
    assert_eq!(request.header("Authorization"), Some("Bearer test-key"));
    assert_eq!(request.json_body()["url"], "https://www.ft.com/content/x");
    assert_eq!(acquisition.capability, ProviderCapability::Fetch);
    assert_eq!(acquisition.envelopes.len(), 1);
    assert_eq!(acquisition.envelopes[0].host, "www.ft.com");
    assert_eq!(
        acquisition.envelopes[0].source_url,
        "https://www.ft.com/content/x"
    );
    assert_eq!(
        acquisition.envelopes[0].text.as_deref(),
        Some("The full article markdown body.")
    );
    assert!(acquisition.envelopes[0].title.is_none());
    // The fetch response carries no cost field and no per-fetch price has been
    // read, so the charge is unknown — never recorded as zero or as a quoted
    // figure nothing quoted.
    assert!(
        acquisition.charge.money.is_none() && acquisition.charge.native.is_none(),
        "an absent fetch cost is unknown, not quoted and not free"
    );
}

/// A `200` whose body lacks `results` (search) or `markdown` (fetch)
/// is a changed shape, not an empty result; the error names the field and no
/// acquisition (so no charge) exists.
#[test]
fn linkup_treats_a_200_without_the_documented_field_as_an_error_naming_it() {
    let origin = Origin::serving(|_| Response::new(200, b"{}".to_vec()));
    let error = LinkupAdapter::new(&origin.url(), "test-key")
        .search("anything", 1, &[])
        .expect_err("a body without `results` must fail");
    assert!(matches!(error, SupplyError::Malformed { .. }), "{error:?}");
    assert!(error.to_string().contains("results"), "{error}");

    let origin = Origin::serving(|_| Response::new(200, b"{}".to_vec()));
    let error = LinkupAdapter::new(&origin.url(), "test-key")
        .fetch("https://www.bbc.co.uk/news/x")
        .expect_err("a fetch body without `markdown` must fail");
    assert!(matches!(error, SupplyError::Malformed { .. }), "{error:?}");
    assert!(error.to_string().contains("markdown"), "{error}");
}
