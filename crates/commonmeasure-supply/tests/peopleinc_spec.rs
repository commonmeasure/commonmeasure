//! Synthetic documented-shape fixtures over the production HTTP transport.
//! These establish fixture verification, not live or recorded replay evidence.

use commonmeasure_http::Response;
use commonmeasure_supply::{
    PeopleIncAdapter, SupplyAdapter, SupplyError, declared_provider_ref, remote_adapter,
    required_variable,
};
use commonmeasure_types::canonical::sha256_digest;
use commonmeasure_types::{LicenceState, ProviderCapability};
use serde_json::{Value, json};

mod common;
use common::Origin;

const KEY: &str = "synthetic-peopleinc-key";
const ARTICLE: &str = "https://www.allrecipes.com/article/meal-prep-guide/";
const MARKDOWN: &str = "# Synthetic recipe\n\nKeep **all** the supplied text.\n\n| Item | Amount |\n| --- | --- |\n| Rice | 1 |\n";

fn search_body() -> Value {
    json!({"question": "q", "documents": [{
        "doc_id": "synthetic-1", "url": ARTICLE, "domain": "Lifestyle", "rank": 1,
        "content": {"markdown": MARKDOWN, "metadata": {
            "title": "Synthetic recipe", "pubdate": "2026-01-01T12:00:00Z", "update": "2026-02-02T12:00:00Z",
            "author": ["Fixture author"], "language": "en-US", "sourcetype": 0,
            "ext": {"bylineBlock": ["Fixture reviewer"]}
        }}
    }], "selection_summary": {"selected_count": 1, "domain_count": 1, "total_wordcount": null}})
}

fn fetch_body() -> Value {
    json!({"status": 200, "markdown": MARKDOWN, "metadata": {"title": "Synthetic recipe", "update": "2026-02-02T12:00:00Z"}})
}

fn origin(body: Value) -> Origin {
    Origin::serving(move |_| Response::new(200, serde_json::to_vec(&body).unwrap()))
}

#[test]
fn search_preserves_full_markdown_order_identity_and_native_evidence() {
    let mut body = search_body();
    let mut second = body["documents"][0].clone();
    second["url"] = json!("https://www.investopedia.com/synthetic-article");
    second["rank"] = json!(2);
    second["doc_id"] = json!("synthetic-2");
    body["documents"].as_array_mut().unwrap().push(second);
    let raw = serde_json::to_vec(&body).unwrap();
    let expected_raw = raw.clone();
    let origin = Origin::serving(move |_| Response::new(200, raw.clone()));
    let got = PeopleIncAdapter::new(&origin.url(), KEY)
        .search("  q  ", 2, &["www.allrecipes.com"])
        .unwrap();
    let request = origin.only_request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/v1/search");
    assert_eq!(request.header("X-API-Key"), Some(KEY));
    assert_eq!(request.header("Content-Type"), Some("application/json"));
    assert_eq!(request.header("Accept-Encoding"), Some("identity"));
    assert_eq!(
        request.json_body(),
        json!({"question":"q", "limit":2, "strategy":"balanced"})
    );
    assert!(!request.target.contains(KEY));
    assert!(!String::from_utf8_lossy(&request.body).contains(KEY));
    assert_eq!(got.raw_response, expected_raw);
    assert_eq!(got.envelopes.len(), 2);
    let first = &got.envelopes[0];
    assert_eq!(first.source_url, ARTICLE);
    assert_eq!(first.host, "www.allrecipes.com");
    assert_eq!(first.text.as_deref(), Some(MARKDOWN));
    assert_eq!(
        first.content_hash.as_deref(),
        Some(sha256_digest(MARKDOWN.as_bytes()).as_str())
    );
    assert_eq!(first.title.as_deref(), Some("Synthetic recipe"));
    assert_eq!(first.retrieval_rank, 1);
    assert_eq!(got.envelopes[1].host, "www.investopedia.com");
    assert_eq!(got.envelopes[1].retrieval_rank, 2);
    assert_eq!(first.native_metadata["doc_id"], "synthetic-1");
    assert_eq!(first.native_metadata["domain"], "Lifestyle");
    assert_eq!(first.native_metadata["rank"], 1);
    assert_eq!(
        first.native_metadata["metadata"]["update"],
        "2026-02-02T12:00:00Z"
    );
    let date = first.declared_date.as_ref().unwrap();
    assert_eq!(date.date, "2026-01-01T12:00:00Z");
    assert!(date.provenance.contains("People Inc metadata.pubdate"));
    assert_eq!(first.licence, LicenceState::Unknown);
    assert!(got.charge.money.is_none() && got.charge.native.is_none());
    assert!(got.provider_request_id.is_none());
    assert!(String::from_utf8_lossy(&got.raw_response).contains("selection_summary"));
}

#[test]
fn validates_trimmed_unicode_questions_before_requests() {
    let origin = origin(json!({"documents":[]}));
    let adapter = PeopleIncAdapter::new(&origin.url(), KEY);
    for question in ["".into(), " \n\t\u{2003}".into(), "é".repeat(2001)] {
        assert!(matches!(
            adapter.search(&question, 1, &[]),
            Err(SupplyError::Malformed { .. })
        ));
    }
    assert!(origin.requests().is_empty());
    let question = "é".repeat(2000);
    adapter.search(&format!(" {question} "), 1, &[]).unwrap();
    assert_eq!(origin.only_request().json_body()["question"], question);
}

#[test]
fn declares_and_applies_five_result_ceiling() {
    for (asked, sent) in [(0, 1), (1, 1), (5, 5), (9, 5), (u32::MAX, 5)] {
        let origin = origin(json!({"documents":[]}));
        let adapter = PeopleIncAdapter::new(&origin.url(), KEY);
        assert_eq!(adapter.maximum_search_results(), Some(5));
        adapter.search("q", asked, &[]).unwrap();
        assert_eq!(origin.only_request().json_body()["limit"], sent);
    }
}

#[test]
fn unexpected_overdelivery_is_refused_without_a_second_paid_request() {
    let mut body = search_body();
    let doc = body["documents"][0].clone();
    body["documents"].as_array_mut().unwrap().push(doc);
    let origin = origin(body);
    assert!(matches!(
        PeopleIncAdapter::new(&origin.url(), KEY).search("q", 1, &[]),
        Err(SupplyError::Malformed { .. })
    ));
    origin.only_request();
}

#[test]
fn absent_or_malformed_documents_and_content_are_explicit_failures() {
    for body in [
        json!({}),
        json!({"documents":null}),
        json!({"documents":{}}),
        json!({"documents":[{}]}),
        json!({"documents":[{"url":ARTICLE}]}),
        json!({"documents":[{"url":ARTICLE,"content":null}]}),
        json!({"documents":[{"url":ARTICLE,"content":{"markdown":false}}]}),
        json!({"documents":[{"url":ARTICLE,"content":{"markdown":" \n "}}]}),
        json!({"documents":[{"url":ARTICLE,"content":{"markdown":"text","metadata":[]}}]}),
    ] {
        let origin = origin(body);
        assert!(matches!(
            PeopleIncAdapter::new(&origin.url(), KEY).search("q", 5, &[]),
            Err(SupplyError::Malformed { .. })
        ));
        origin.only_request();
    }
}

#[test]
fn empty_documents_are_successful_and_optional_metadata_stays_absent() {
    let empty = origin(json!({"documents":[]}));
    assert!(
        PeopleIncAdapter::new(&empty.url(), KEY)
            .search("q", 5, &[])
            .unwrap()
            .envelopes
            .is_empty()
    );
    let origin = origin(json!({"documents":[{"url":ARTICLE,"content":{"markdown":"text"}}]}));
    let got = PeopleIncAdapter::new(&origin.url(), KEY)
        .search("q", 5, &[])
        .unwrap();
    assert!(got.envelopes[0].title.is_none());
    assert!(got.envelopes[0].declared_date.is_none());
    assert!(got.envelopes[0].native_metadata.get("metadata").is_none());
}

#[test]
fn fetch_encodes_the_exact_https_target_and_preserves_trailing_slash() {
    for target in [ARTICLE, "https://www.investopedia.com/é%20&=fixture"] {
        let origin = origin(fetch_body());
        let got = PeopleIncAdapter::new(&origin.url(), KEY)
            .fetch(target)
            .unwrap();
        let request = origin.only_request();
        assert_eq!(request.method, "GET");
        assert_eq!(request.header("X-API-Key"), Some(KEY));
        assert!(request.header("If-None-Match").is_none());
        let called = url::Url::parse(&format!("{}{}", origin.url(), request.target)).unwrap();
        assert_eq!(called.path(), "/v1/markdown");
        assert_eq!(
            called.query_pairs().collect::<Vec<_>>(),
            vec![("url".into(), target.into())]
        );
        assert!(!request.target.contains(KEY));
        assert!(request.body.is_empty());
        assert_eq!(got.envelopes[0].source_url, target);
        assert_eq!(got.envelopes[0].text.as_deref(), Some(MARKDOWN));
        assert_eq!(
            got.envelopes[0].declared_date.as_ref().unwrap().provenance,
            "supplier-declared: People Inc metadata.update"
        );
        assert_eq!(got.capability, ProviderCapability::Fetch);
    }
}

#[test]
fn invalid_targets_are_refused_without_canonical_substitution_or_request() {
    let origin = origin(fetch_body());
    let adapter = PeopleIncAdapter::new(&origin.url(), KEY);
    for target in [
        "not-a-url",
        "http://www.allrecipes.com/a",
        "https://www.allrecipes.com/a?q=1",
        "https://www.allrecipes.com/a#part",
        "https://user:pass@www.allrecipes.com/a",
        "https://www.allrecipes.com:8443/a",
        " https://www.allrecipes.com/a",
        "https://www.allrecipes.com/with space",
        "https://www.allrecipes.com\\a",
    ] {
        assert!(
            matches!(adapter.fetch(target), Err(SupplyError::Malformed { .. })),
            "{target}"
        );
    }
    assert!(origin.requests().is_empty());
}

#[test]
fn fetch_missing_or_malformed_markdown_is_not_a_successful_empty_body() {
    for body in [
        json!({}),
        json!({"status":200}),
        json!({"status":200,"markdown":0}),
        json!({"status":200,"markdown":""}),
        json!({"status":403,"markdown":"text"}),
    ] {
        let origin = origin(body);
        assert!(matches!(
            PeopleIncAdapter::new(&origin.url(), KEY).fetch(ARTICLE),
            Err(SupplyError::Malformed { .. })
        ));
        origin.only_request();
    }
}

#[test]
fn problem_details_validation_request_id_and_retry_after_survive_errors() {
    for status in [401, 403, 404, 422, 429, 500, 503] {
        let problem = json!({"type":"about:blank", "status":status, "title":"Fixture error",
            "detail":"Fixture explanation", "instance":"/v1/markdown?url=fixture",
            "errors":[{"path":["url"], "message":"x".repeat(800), "code":"invalid_format"}],
            "redirectUrl":"/v1/markdown?url=https%3A%2F%2Fwww.allrecipes.com%2Freplacement",
            "requestId":"synthetic-request-id"});
        let expected = problem.clone();
        let origin = Origin::serving(move |_| {
            let mut response = Response::new(status, serde_json::to_vec(&problem).unwrap());
            response.headers.set("Retry-After", "17");
            response
        });
        let error = PeopleIncAdapter::new(&origin.url(), KEY)
            .fetch(ARTICLE)
            .unwrap_err();
        let SupplyError::Status {
            status: got_status,
            detail,
            ..
        } = error
        else {
            panic!("status error")
        };
        assert_eq!(got_status, status);
        let evidence: Value = serde_json::from_str(&detail).unwrap();
        assert_eq!(evidence["problem"], expected);
        assert_eq!(evidence["retry_after"], "17");
        assert_eq!(evidence["attempts"], 1);
        assert_eq!(evidence["automatic_retry"], false);
        origin.only_request();
    }
}

#[test]
fn search_errors_are_terminal_too() {
    for status in [401, 422, 429, 500] {
        let origin =
            Origin::serving(move |_| Response::json(status, r#"{"detail":"fixture failure"}"#));
        assert!(
            matches!(PeopleIncAdapter::new(&origin.url(), KEY).search("q", 5, &[]), Err(SupplyError::Status {status: got, ..}) if got == status)
        );
        origin.only_request();
    }
}

#[test]
fn moved_articles_and_http_redirects_never_fetch_the_candidate() {
    let destination = origin(fetch_body());
    for status in [301, 302, 303, 307, 308, 404] {
        let candidate = format!("{}/v1/markdown?url={ARTICLE}", destination.url());
        let expected = candidate.clone();
        let origin = Origin::serving(move |_| {
            let mut response =
                Response::json(status, &json!({"redirectUrl":candidate}).to_string());
            response.headers.set("Location", &candidate);
            response
        });
        let error = PeopleIncAdapter::new(&origin.url(), KEY)
            .fetch(ARTICLE)
            .unwrap_err();
        assert!(error.to_string().contains(&expected));
        assert!(error.to_string().contains("requires confirmation"));
        origin.only_request();
    }
    assert!(destination.requests().is_empty());
}

#[test]
fn unconditional_fetches_ignore_etag_and_refuse_empty_304() {
    let origin = Origin::serving(|_| {
        let mut response = Response::new(200, serde_json::to_vec(&fetch_body()).unwrap());
        response.headers.set("ETag", "\"synthetic-tag\"");
        response
    });
    let adapter = PeopleIncAdapter::new(&origin.url(), KEY);
    adapter.fetch(ARTICLE).unwrap();
    adapter.fetch(ARTICLE).unwrap();
    assert_eq!(origin.requests().len(), 2);
    assert!(
        origin
            .requests()
            .iter()
            .all(|request| request.header("If-None-Match").is_none())
    );
    let not_modified = Origin::serving(|_| Response::new(304, vec![]));
    let error = PeopleIncAdapter::new(&not_modified.url(), KEY)
        .fetch(ARTICLE)
        .unwrap_err();
    assert!(matches!(error, SupplyError::Status { status: 304, .. }));
    assert!(error.to_string().contains("no authorised cached body"));
    not_modified.only_request();
}

#[test]
fn credential_echoes_are_redacted_on_errors_and_never_sealed_on_success() {
    for status in [200, 403] {
        let mut body = fetch_body();
        body["requestId"] = json!(KEY);
        let origin =
            Origin::serving(move |_| Response::new(status, serde_json::to_vec(&body).unwrap()));
        let error = PeopleIncAdapter::new(&origin.url(), KEY)
            .fetch(ARTICLE)
            .unwrap_err();
        assert!(!error.to_string().contains(KEY));
        assert!(!format!("{error:?}").contains(KEY));
    }
    let key = "synthetic-\"quoted\\key";
    let body = json!({"status":200,"markdown":"text","metadata":{"echo":key}});
    let origin = origin(body);
    let error = PeopleIncAdapter::new(&origin.url(), key)
        .fetch(ARTICLE)
        .unwrap_err();
    assert!(!error.to_string().contains(key));
    assert!(error.to_string().contains("echoed the credential"));
}

#[test]
fn json_unicode_escapes_cannot_hide_a_credential_echo_in_sealed_evidence() {
    let escaped_key: String = KEY
        .chars()
        .map(|c| format!("\\u{:04x}", c as u32))
        .collect();
    let body =
        format!(r#"{{"status":200,"markdown":"text","metadata":{{"echo":"{escaped_key}"}}}}"#);
    assert!(!body.contains(KEY));
    let origin = Origin::serving(move |_| Response::new(200, body.as_bytes().to_vec()));
    let error = PeopleIncAdapter::new(&origin.url(), KEY)
        .fetch(ARTICLE)
        .unwrap_err();
    assert!(error.to_string().contains("echoed the credential"));
    assert!(!error.to_string().contains(KEY));
}

#[test]
fn transport_faults_cannot_echo_the_key() {
    let origin = Origin::serving(|_| {
        let mut response = Response::new(200, vec![]);
        response.headers.set("Content-Encoding", KEY);
        response
    });
    let error = PeopleIncAdapter::new(&origin.url(), KEY)
        .fetch(ARTICLE)
        .unwrap_err();
    assert!(matches!(error, SupplyError::Transport { .. }));
    assert!(!error.to_string().contains(KEY));
    assert!(!format!("{error:?}").contains(KEY));
}

#[test]
fn provider_registration_uses_normal_credential_and_capability_paths() {
    assert_eq!(required_variable("peopleinc"), Some("PEOPLEINC_API_KEY"));
    let reference = declared_provider_ref("peopleinc").unwrap();
    assert_eq!(
        reference.capabilities,
        vec![ProviderCapability::Search, ProviderCapability::Fetch]
    );
    let origin = origin(search_body());
    let adapter = remote_adapter("peopleinc", Some(&origin.url()), KEY).unwrap();
    assert_eq!(adapter.provider(), "peopleinc");
    assert_eq!(adapter.maximum_search_results(), Some(5));
    adapter.search("q", 5, &[]).unwrap();
    origin.only_request();
}

#[test]
fn blank_credentials_are_unavailable_before_transport() {
    let origin = origin(search_body());
    assert!(
        matches!(PeopleIncAdapter::new(&origin.url(), " ").search("q", 5, &[]), Err(SupplyError::CredentialMissing {variable}) if variable == "PEOPLEINC_API_KEY")
    );
    assert!(origin.requests().is_empty());
}
