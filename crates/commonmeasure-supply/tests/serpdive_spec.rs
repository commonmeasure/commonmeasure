//! The SERPdive adapter, over the real transport, against its documented
//! response shape.
//!
//! SERPdive's verification state and the artefacts behind it are recorded in
//! `docs/knowledge-base/provider-verification.md`; no recon capture is
//! committed, so these tests need no credential and exercise the parser over
//! SERPdive's documented shapes (`https://serpdive.com/docs`, read 18 August 2026) through the real
//! transport and a loopback origin. Nothing in the path is a stand-in — the
//! client, the framing, the parser and the charge rules are the ones a live
//! call would use. Only the origin is local, which is the substitution
//! `AGENTS.md` permits.
//!
//! Each `tests/*.rs` is its own test crate, so the small loopback-origin helper
//! is copied here from the `recorded_replay.rs` pattern rather than shared.

use commonmeasure_http::Response;
use commonmeasure_supply::{SerpdiveAdapter, SupplyAdapter, SupplyError};
use commonmeasure_types::{ChargeBasis, LicenceState};
mod common;

use common::Origin;

/// The request must reach the documented endpoint with Bearer auth and the
/// query in the documented field, and the reply's `results` must become
/// envelopes: `content` the text, `date` the declared date, and the published
/// one-credit `mako` price the quoted charge. A target publisher domain is used
/// for realism — this harness exists to probe exactly these hosts.
#[test]
fn serpdive_search_parses_its_documented_shape() {
    let origin = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({
                "query": "AI copyright ruling",
                "model": "mako",
                "response_time_ms": 2641,
                "results": [
                    {
                        "url": "https://www.theguardian.com/technology/2026/aug/01/ai-copyright",
                        "title": "A copyright ruling",
                        "date": "2026-08-01",
                        "content": "The court found in favour of the publishers."
                    },
                    {
                        "url": "https://www.ft.com/content/undated-piece",
                        "title": "Untimed analysis",
                        "content": ""
                    }
                ]
            }))
            .unwrap(),
        )
    });

    let acquisition = SerpdiveAdapter::new(&origin.url(), "test-key")
        .search("AI copyright ruling", 5, &[])
        .expect("documented shape");

    let request = origin.only_request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/v1/search");
    assert_eq!(request.header("Authorization"), Some("Bearer test-key"));
    assert_eq!(request.json_body()["query"], "AI copyright ruling");
    assert_eq!(request.json_body()["model"], "mako");
    assert_eq!(request.json_body()["max_results"], 5);

    assert_eq!(acquisition.provider, "serpdive");
    assert_eq!(acquisition.http_status, Some(200));
    // SERPdive returns no request identifier anywhere in the response.
    assert!(acquisition.provider_request_id.is_none());
    assert_eq!(acquisition.envelopes.len(), 2);

    let first = &acquisition.envelopes[0];
    assert_eq!(first.host, "www.theguardian.com");
    assert_eq!(
        first.source_url,
        "https://www.theguardian.com/technology/2026/aug/01/ai-copyright"
    );
    assert_eq!(first.title.as_deref(), Some("A copyright ruling"));
    assert_eq!(first.retrieval_rank, 1);
    assert_eq!(
        first.text.as_deref(),
        Some("The court found in favour of the publishers.")
    );
    assert!(
        first
            .content_hash
            .as_deref()
            .unwrap()
            .starts_with("sha256:"),
        "admitted text must be hashed at ingestion"
    );
    // Search accessibility is not permission, so the envelope says unknown.
    assert_eq!(first.licence, LicenceState::Unknown);
    let dated = first.declared_date.as_ref().expect("date maps");
    assert_eq!(dated.date, "2026-08-01");
    assert!(
        dated.provenance.contains("supplier-declared") && dated.provenance.contains("SERPdive"),
        "the provenance must name who declared the date: {}",
        dated.provenance
    );
    // Promoted fields do not reappear under their own names.
    assert!(first.native_metadata.get("content").is_none());
    assert!(first.native_metadata.get("date").is_none());

    // A result with empty content and no date carries neither text nor date.
    let second = &acquisition.envelopes[1];
    assert!(second.text.is_none());
    assert!(second.content_hash.is_none());
    assert!(second.declared_date.is_none());

    // No cost field on the response: the charge is quoted from the published
    // one-credit mako price, never observed, and never given a currency.
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

/// SERPdive documents no domain include filter, so a job's allowed hosts change
/// nothing about the request: whether or not hosts are named, the body carries
/// the same three documented fields and no scoping field appears. Admission
/// enforces the host policy over the results instead.
#[test]
fn serpdive_sends_no_domain_filter_because_the_provider_documents_none() {
    let with_hosts = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({"results": []})).unwrap(),
        )
    });
    SerpdiveAdapter::new(&with_hosts.url(), "test-key")
        .search("anything", 3, &["www.theguardian.com", "ft.com"])
        .expect("documented shape");
    let body = with_hosts.only_request().json_body();
    let object = body.as_object().expect("the request body is a JSON object");
    // The body carries exactly the documented fields and nothing that looks
    // like a domain filter, whatever it might have been named.
    assert_eq!(
        object.len(),
        3,
        "only query, model and max_results are sent"
    );
    for key in ["query", "model", "max_results"] {
        assert!(
            object.contains_key(key),
            "the documented field {key} is sent"
        );
    }
    for absent in [
        "include_domains",
        "includeDomains",
        "domains",
        "sources",
        "source_policy",
    ] {
        assert!(
            object.get(absent).is_none(),
            "no undocumented domain filter {absent} may be invented"
        );
    }

    // An unscoped search sends a byte-identical body.
    let unscoped = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({"results": []})).unwrap(),
        )
    });
    SerpdiveAdapter::new(&unscoped.url(), "test-key")
        .search("anything", 3, &[])
        .expect("documented shape");
    assert_eq!(
        unscoped.only_request().json_body(),
        body,
        "naming hosts must not change the request, since the provider has no filter"
    );
}

/// `max_results` is documented as 1–10; a job limit above that window is
/// clamped so the request never asks the provider for a count it would reject.
#[test]
fn serpdive_clamps_max_results_into_the_documented_range() {
    let origin = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({"results": []})).unwrap(),
        )
    });
    SerpdiveAdapter::new(&origin.url(), "test-key")
        .search("anything", 50, &[])
        .expect("documented shape");
    assert_eq!(
        origin.only_request().json_body()["max_results"],
        10,
        "a limit above the documented range is clamped to 10"
    );
}

/// A `200` whose body lacks `results` is a changed shape, not an empty
/// search; the error names the field and no acquisition (so no charge) exists.
#[test]
fn serpdive_treats_a_200_without_results_as_an_error_naming_the_field() {
    let origin = Origin::serving(|_| Response::new(200, b"{}".to_vec()));

    let error = SerpdiveAdapter::new(&origin.url(), "test-key")
        .search("anything", 1, &[])
        .expect_err("a body without `results` must fail");
    assert!(matches!(error, SupplyError::Malformed { .. }), "{error:?}");
    assert!(error.to_string().contains("results"), "{error}");
}
