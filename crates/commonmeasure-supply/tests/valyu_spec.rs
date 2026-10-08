//! Documented Valyu OpenAPI 2.3.0 shapes through the production transport.
//! Synthetic bodies establish request construction and parsing, not live or
//! recorded integration. No credential is needed.

use commonmeasure_http::Response;
use commonmeasure_supply::{
    SupplyAdapter, SupplyError, ValyuAdapter, declared_provider_ref, remote_adapter,
    required_variable,
};
use commonmeasure_types::{ChargeBasis, LicenceState, ProviderCapability};
use serde_json::{Value, json};

mod common;
use common::Origin;

fn result() -> Value {
    json!({
        "id": "document-id", "title": "A standard", "url": "https://www.w3.org/TR/prov-dm/",
        "content": "# A standard\n\nA passage.", "description": "A description, not the content.",
        "source": "web", "source_type": "website", "data_type": "unstructured",
        "price": 0.0015, "length": 25, "relevance_score": 0.97,
        "publication_date": "2013-04-30", "crawl_date": "2026-10-06",
        "image_url": {"0": "https://www.w3.org/image.png"}
    })
}

fn search_response() -> Value {
    json!({
        "success": true, "error": null, "tx_id": "tx-search", "query": "provenance",
        "results": [result()], "results_by_source": {"web": 1, "proprietary": 0},
        "total_deduction_dollars": 0.0045, "total_characters": 25
    })
}

fn serve(status: u16, body: Value) -> Origin {
    Origin::serving(move |_| Response::new(status, serde_json::to_vec(&body).unwrap()))
}

#[test]
fn valyu_search_preserves_provenance_content_and_observed_prices() {
    let response = search_response();
    let raw = serde_json::to_vec(&response).unwrap();
    let origin = serve(200, response);
    let acquisition = ValyuAdapter::new(&origin.url(), "synthetic-key")
        .search("provenance", 3, &[])
        .unwrap();
    let request = origin.only_request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/v1/search");
    assert_eq!(request.header("x-api-key"), Some("synthetic-key"));
    assert_eq!(request.header("Accept-Encoding"), Some("identity"));
    assert_eq!(
        request.json_body(),
        json!({
            "query": "provenance", "search_type": "all", "max_num_results": 3, "is_tool_call": true
        })
    );
    assert_eq!(acquisition.raw_response, raw);
    assert_eq!(acquisition.provider, "valyu");
    assert_eq!(acquisition.capability, ProviderCapability::Search);
    assert_eq!(
        acquisition.provider_request_id.as_deref(),
        Some("tx-search")
    );
    assert_eq!(acquisition.charge.money.unwrap().micros, 4500);
    let native = acquisition.charge.native.unwrap();
    assert_eq!(native.unit, "USD");
    assert_eq!(native.basis, ChargeBasis::Observed);
    assert_eq!(native.amount, json!(0.0045).as_number().unwrap().clone());
    let envelope = &acquisition.envelopes[0];
    assert_eq!(envelope.host, "www.w3.org");
    assert_eq!(envelope.text.as_deref(), Some("# A standard\n\nA passage."));
    assert_eq!(
        envelope.content_hash.as_deref(),
        Some(commonmeasure_types::canonical::sha256_digest(b"# A standard\n\nA passage.").as_str())
    );
    assert_eq!(envelope.licence, LicenceState::Unknown);
    assert_eq!(envelope.retrieval_rank, 1);
    let date = envelope.declared_date.as_ref().unwrap();
    assert_eq!(date.date, "2013-04-30");
    assert!(date.provenance.contains("Valyu"));
    assert!(date.provenance.contains("publication_date"));
    for field in [
        "id",
        "description",
        "source",
        "source_type",
        "data_type",
        "price",
        "length",
        "relevance_score",
        "crawl_date",
        "image_url",
    ] {
        assert_eq!(envelope.native_metadata[field], result()[field], "{field}");
    }
    for field in ["title", "url", "content", "publication_date"] {
        assert!(envelope.native_metadata.get(field).is_none(), "{field}");
    }
}

#[test]
fn valyu_scopes_every_host_and_declares_and_clamps_the_standard_ceiling() {
    for (requested, sent) in [(0, 1), (3, 3), (100, 20)] {
        let origin = serve(200, search_response());
        let adapter = ValyuAdapter::new(&origin.url(), "synthetic-key");
        assert_eq!(adapter.maximum_search_results(), Some(20));
        adapter
            .search("provenance", requested, &["www.w3.org", "arxiv.org"])
            .unwrap();
        let body = origin.only_request().json_body();
        assert_eq!(body["max_num_results"], sent);
        assert_eq!(body["included_sources"], json!(["www.w3.org", "arxiv.org"]));
    }
}

#[test]
fn valyu_fetch_uses_one_url_its_own_cost_field_and_only_successful_items() {
    let mut served = result();
    served["status"] = json!("success");
    served["price"] = json!(0.001);
    served["render_method"] = json!("http");
    served["last_modified"] = json!("Fri, 10 May 2024 16:34:56 GMT");
    let body = json!({
        "success": true, "tx_id": "tx-contents", "urls_requested": 2, "urls_processed": 1,
        "urls_failed": 1, "results": [
            {"url": "https://failed.example/", "status": "failed", "error": "unreachable",
             "content": "must not become supply"}, served
        ],
        "total_cost_dollars": 0.001,
        "total_deduction_dollars": 9.99, "total_characters": 25
    });
    for status in [200, 206] {
        let origin = serve(status, body.clone());
        let acquisition = ValyuAdapter::new(&origin.url(), "synthetic-key")
            .fetch("https://www.w3.org/TR/prov-dm/")
            .unwrap();
        let request = origin.only_request();
        assert_eq!(request.target, "/v1/contents");
        assert_eq!(request.header("x-api-key"), Some("synthetic-key"));
        assert_eq!(
            request.json_body(),
            json!({
                "urls": ["https://www.w3.org/TR/prov-dm/"], "response_length": "medium"
            })
        );
        assert_eq!(acquisition.capability, ProviderCapability::Fetch);
        assert_eq!(acquisition.http_status, Some(status));
        assert_eq!(
            acquisition.provider_request_id.as_deref(),
            Some("tx-contents")
        );
        assert_eq!(acquisition.charge.money.unwrap().micros, 1000);
        assert_eq!(
            acquisition.charge.native.unwrap().basis,
            ChargeBasis::Observed
        );
        assert_eq!(acquisition.envelopes.len(), 1);
        let envelope = &acquisition.envelopes[0];
        assert_eq!(envelope.retrieval_rank, 2);
        assert_eq!(envelope.native_metadata["price"], json!(0.001));
        assert_eq!(envelope.native_metadata["render_method"], "http");
        assert_eq!(
            envelope.native_metadata["last_modified"],
            "Fri, 10 May 2024 16:34:56 GMT"
        );
        assert_eq!(envelope.licence, LicenceState::Unknown);
        assert!(String::from_utf8_lossy(&acquisition.raw_response).contains("unreachable"));
    }
}

#[test]
fn valyu_missing_cost_is_unknown_even_with_per_result_prices_or_the_other_total() {
    for cost in [Value::Null, json!("0.0045")] {
        let mut body = search_response();
        body["total_deduction_dollars"] = cost;
        body["total_cost_dollars"] = json!(1);
        let origin = serve(200, body);
        let acquisition = ValyuAdapter::new(&origin.url(), "synthetic-key")
            .search("q", 1, &[])
            .unwrap();
        assert!(acquisition.charge.money.is_none());
        assert!(acquisition.charge.native.is_none());
    }
    let mut body = search_response();
    body.as_object_mut()
        .unwrap()
        .remove("total_deduction_dollars");
    let origin = serve(200, body);
    assert!(
        ValyuAdapter::new(&origin.url(), "synthetic-key")
            .search("q", 1, &[])
            .unwrap()
            .charge
            .money
            .is_none()
    );
}

#[test]
fn valyu_preserves_uncomparable_native_charge_and_observed_zero() {
    for (cost, micros) in [(json!(0.0000001), None), (json!(0), Some(0))] {
        let mut body = search_response();
        body["total_deduction_dollars"] = cost.clone();
        let origin = serve(200, body);
        let acquisition = ValyuAdapter::new(&origin.url(), "synthetic-key")
            .search("q", 1, &[])
            .unwrap();
        assert_eq!(acquisition.charge.money.map(|money| money.micros), micros);
        let native = acquisition.charge.native.unwrap();
        assert_eq!(&native.amount, cost.as_number().unwrap());
        assert_eq!(native.note.is_some(), micros.is_none());
    }
}

#[test]
fn valyu_empty_and_missing_fields_stay_distinct() {
    let mut body = search_response();
    body["results"] = json!([]);
    let origin = serve(206, body);
    assert!(
        ValyuAdapter::new(&origin.url(), "synthetic-key")
            .search("q", 1, &[])
            .unwrap()
            .envelopes
            .is_empty()
    );
    for body in [
        json!({"success": true}),
        json!({"success": true, "results": null}),
        json!({"results": []}),
        json!({"success": "true", "results": []}),
    ] {
        let origin = serve(200, body);
        assert!(matches!(
            ValyuAdapter::new(&origin.url(), "synthetic-key").search("q", 1, &[]),
            Err(SupplyError::Malformed { .. })
        ));
    }
    let mut body = search_response();
    body["results"][0]["title"] = json!("");
    body["results"][0]["content"] = json!("");
    body["results"][0]["publication_date"] = json!("");
    body["results"]
        .as_array_mut()
        .unwrap()
        .push(json!({"url": "not-a-url", "content": "discard"}));
    let origin = serve(200, body);
    let acquisition = ValyuAdapter::new(&origin.url(), "synthetic-key")
        .search("q", 2, &[])
        .unwrap();
    assert_eq!(acquisition.envelopes.len(), 1);
    let envelope = &acquisition.envelopes[0];
    assert!(
        envelope.text.is_none()
            && envelope.title.is_none()
            && envelope.declared_date.is_none()
            && envelope.content_hash.is_none()
    );
}

#[test]
fn valyu_failures_are_explicit_and_never_retried() {
    for status in [200, 401, 402, 422, 429, 500] {
        let origin = serve(
            status,
            json!({"success": false, "error": "provider refused", "results": []}),
        );
        let error = ValyuAdapter::new(&origin.url(), "synthetic-key")
            .fetch("https://www.w3.org/")
            .unwrap_err();
        assert!(
            matches!(error, SupplyError::Status { status: actual, ref detail, .. } if actual == status && detail.contains("provider refused"))
        );
        origin.only_request();
    }
    for status in [Value::Null, json!("pending")] {
        let origin = serve(
            206,
            json!({"success": true, "results": [{"url": "https://www.w3.org/", "status": status}]}),
        );
        assert!(matches!(
            ValyuAdapter::new(&origin.url(), "synthetic-key").fetch("https://www.w3.org/"),
            Err(SupplyError::Malformed { .. })
        ));
    }
}

#[test]
fn valyu_registration_names_only_search_fetch_and_its_credential() {
    assert!(commonmeasure_supply::IMPLEMENTED_PROVIDERS.contains(&"valyu"));
    assert_eq!(required_variable("valyu"), Some("VALYU_API_KEY"));
    let declared = declared_provider_ref("valyu").unwrap();
    assert_eq!(
        declared.capabilities,
        [ProviderCapability::Search, ProviderCapability::Fetch]
    );
    let origin = serve(200, search_response());
    let adapter = remote_adapter("valyu", Some(&origin.url()), "synthetic-key").unwrap();
    assert_eq!(adapter.provider(), "valyu");
    assert_eq!(adapter.capabilities(), declared.capabilities);
    assert!(matches!(
        adapter.invoke("q"),
        Err(SupplyError::CapabilityUnavailable { .. })
    ));
    adapter.search("q", 1, &[]).unwrap();
    assert_eq!(origin.only_request().target, "/v1/search");
}
