//! Dataville's authenticated wiki and arxiv responses recorded on 25 September
//! 2026, served on loopback through the production transport and parser.
//!
//! Body and abstract text keep a 120-character lead and a marker with
//! the original length, as in recorded_replay.rs. These are parser fixtures,
//! not a claim that a full run has replayed the original content. Error and
//! stale fixtures below follow the API reference or explicitly inject faults.

use commonmeasure_http::Response;
use commonmeasure_supply::{
    DatavilleAdapter, SupplyAdapter, SupplyError, declared_provider_ref, remote_adapter,
    required_variable,
};
use commonmeasure_types::{ChargeBasis, LicenceState, ProviderCapability};
use serde_json::{Value, json};
mod common;
use common::Origin;

fn recorded_wiki() -> Value {
    serde_json::from_str(r##"{
  "account_state": "authenticated",
  "data": {
    "attribution": {
      "attribution": "Wikipedia contributors — CC-BY-SA-4.0",
      "license": {
        "identifier": "CC-BY-SA-4.0",
        "name": "Creative Commons Attribution-ShareAlike License 4.0",
        "url": "https://creativecommons.org/licenses/by-sa/4.0/"
      },
      "notice": "This content comes from Wikipedia, a Wikimedia project. If you redistribute it you must credit Wikimedia as the source, comply with CC-BY-SA-4.0, including its share-alike requirement, and follow Wikimedia's trademark policy and visual identity guidelines when using Wikimedia names or marks.",
      "project": "Wikipedia",
      "source": "Wikimedia",
      "terms_of_use": "https://foundation.wikimedia.org/wiki/Policy:Terms_of_Use",
      "trademark_policy": "https://foundation.wikimedia.org/wiki/Trademark_policy",
      "url": "https://en.wikipedia.org/wiki/Apollo_11",
      "visual_identity_guidelines": "https://foundation.wikimedia.org/wiki/Visual_identity_guidelines"
    },
    "body": "Apollo 11\n\nFirst crewed Moon landing (1969)\n\n\"First Moon landing\" and \"The Moon landing\" redirect here. For other crewed [elided; 156283 characters, 157167 bytes total]",
    "entities": [],
    "id": "wiki_en_apollo_11",
    "language": "en",
    "last_updated": "2026-08-24T18:22:02.000Z",
    "metadata": {
      "abstract": "Apollo 11 was the American spaceflight that first landed humans on the Moon, and the fifth crewed mission of NASA's Apol [elided; 3215 characters, 3219 bytes total]",
      "categories": [
        {
          "name": "Category:Wikipedia indefinitely semi-protected pages",
          "url": "https://en.wikipedia.org/wiki/Category:Wikipedia_indefinitely_semi-protected_pages"
        },
        {
          "name": "Category:CS1: long volume value",
          "url": "https://en.wikipedia.org/wiki/Category:CS1:_long_volume_value"
        },
        {
          "name": "Category:Webarchive template wayback links",
          "url": "https://en.wikipedia.org/wiki/Category:Webarchive_template_wayback_links"
        },
        {
          "name": "Category:Buzz Aldrin",
          "url": "https://en.wikipedia.org/wiki/Category:Buzz_Aldrin"
        },
        {
          "name": "Category:Wikipedia indefinitely move-protected pages",
          "url": "https://en.wikipedia.org/wiki/Category:Wikipedia_indefinitely_move-protected_pages"
        },
        {
          "name": "Category:Wikisource templates with missing id",
          "url": "https://en.wikipedia.org/wiki/Category:Wikisource_templates_with_missing_id"
        },
        {
          "name": "Category:Neil Armstrong",
          "url": "https://en.wikipedia.org/wiki/Category:Neil_Armstrong"
        },
        {
          "name": "Category:Spacecraft that soft-landed on the Moon",
          "url": "https://en.wikipedia.org/wiki/Category:Spacecraft_that_soft-landed_on_the_Moon"
        },
        {
          "name": "Category:Short description is different from Wikidata",
          "url": "https://en.wikipedia.org/wiki/Category:Short_description_is_different_from_Wikidata"
        },
        {
          "name": "Category:Use American English from July 2019",
          "url": "https://en.wikipedia.org/wiki/Category:Use_American_English_from_July_2019"
        },
        {
          "name": "Category:LQ12 quadrangle",
          "url": "https://en.wikipedia.org/wiki/Category:LQ12_quadrangle"
        },
        {
          "name": "Category:Articles containing potentially dated statements from 2025",
          "url": "https://en.wikipedia.org/wiki/Category:Articles_containing_potentially_dated_statements_from_2025"
        },
        {
          "name": "Category:All articles lacking reliable references",
          "url": "https://en.wikipedia.org/wiki/Category:All_articles_lacking_reliable_references"
        },
        {
          "name": "Category:Articles lacking reliable references from November 2025",
          "url": "https://en.wikipedia.org/wiki/Category:Articles_lacking_reliable_references_from_November_2025"
        },
        {
          "name": "Category:Spoken articles",
          "url": "https://en.wikipedia.org/wiki/Category:Spoken_articles"
        },
        {
          "name": "Category:1969 on the Moon",
          "url": "https://en.wikipedia.org/wiki/Category:1969_on_the_Moon"
        },
        {
          "name": "Category:Articles with short description",
          "url": "https://en.wikipedia.org/wiki/Category:Articles_with_short_description"
        },
        {
          "name": "Category:All articles containing potentially dated statements",
          "url": "https://en.wikipedia.org/wiki/Category:All_articles_containing_potentially_dated_statements"
        },
        {
          "name": "Category:Crewed Apollo missions",
          "url": "https://en.wikipedia.org/wiki/Category:Crewed_Apollo_missions"
        },
        {
          "name": "Category:Articles containing video clips",
          "url": "https://en.wikipedia.org/wiki/Category:Articles_containing_video_clips"
        },
        {
          "name": "Category:Featured articles",
          "url": "https://en.wikipedia.org/wiki/Category:Featured_articles"
        },
        {
          "name": "Category:Use mdy dates from March 2025",
          "url": "https://en.wikipedia.org/wiki/Category:Use_mdy_dates_from_March_2025"
        },
        {
          "name": "Category:Pages using gadget WikiMiniAtlas",
          "url": "https://en.wikipedia.org/wiki/Category:Pages_using_gadget_WikiMiniAtlas"
        },
        {
          "name": "Category:Articles with hAudio microformats",
          "url": "https://en.wikipedia.org/wiki/Category:Articles_with_hAudio_microformats"
        },
        {
          "name": "Category:Pages using multiple image with auto scaled images",
          "url": "https://en.wikipedia.org/wiki/Category:Pages_using_multiple_image_with_auto_scaled_images"
        },
        {
          "name": "Category:NASA crewed missions to the Moon",
          "url": "https://en.wikipedia.org/wiki/Category:NASA_crewed_missions_to_the_Moon"
        },
        {
          "name": "Category:Spacecraft launched by Saturn rockets",
          "url": "https://en.wikipedia.org/wiki/Category:Spacecraft_launched_by_Saturn_rockets"
        },
        {
          "name": "Category:All Wikipedia articles written in American English",
          "url": "https://en.wikipedia.org/wiki/Category:All_Wikipedia_articles_written_in_American_English"
        },
        {
          "name": "Category:Articles with Internet Archive links",
          "url": "https://en.wikipedia.org/wiki/Category:Articles_with_Internet_Archive_links"
        },
        {
          "name": "Category:Successful space missions",
          "url": "https://en.wikipedia.org/wiki/Category:Successful_space_missions"
        },
        {
          "name": "Category:Apollo 11",
          "url": "https://en.wikipedia.org/wiki/Category:Apollo_11"
        },
        {
          "name": "Category:Michael Collins (astronaut)",
          "url": "https://en.wikipedia.org/wiki/Category:Michael_Collins_(astronaut)"
        }
      ],
      "description": null,
      "image": null,
      "infoboxes": null,
      "license": [
        {
          "identifier": "CC-BY-SA-4.0",
          "name": "Creative Commons Attribution-ShareAlike License 4.0",
          "url": "https://creativecommons.org/licenses/by-sa/4.0/"
        }
      ],
      "main_entity": {
        "identifier": "Q43653",
        "url": "https://www.wikidata.org/entity/Q43653"
      },
      "pageid": 662,
      "project": "enwiki",
      "served_from": "snapshot",
      "snapshot_date": "2026-09-10T00:00:00.000Z",
      "url": "https://en.wikipedia.org/wiki/Apollo_11"
    },
    "source": "wikipedia",
    "title": "Apollo 11"
  },
  "status": "success",
  "usage": {
    "credits_remaining": 4.95,
    "credits_total": 5,
    "credits_used": 0.05,
    "request_cost": 0.03,
    "request_limit": 100000,
    "requests_remaining": 99998
  }
}"##).unwrap()
}

fn recorded_arxiv() -> Value {
    serde_json::from_str(r##"{
  "account_state": "authenticated",
  "data": {
    "body": "Authors: Luke Melas-Kyriazi\nPublished: 2021-05-06\nCategories: cs.CV\n\nThe strong performance of vision transformers on im [elided; 1190 characters, 1190 bytes total]",
    "entities": [
      "Luke Melas-Kyriazi"
    ],
    "id": "arxiv_2105.02723",
    "language": "en",
    "last_updated": "2021-05-06T14:42:39Z",
    "metadata": {
      "abs_url": "http://arxiv.org/abs/2105.02723v1",
      "arxiv_id": "2105.02723",
      "authors": [
        "Luke Melas-Kyriazi"
      ],
      "categories": [
        "cs.CV"
      ],
      "pdf_url": null,
      "published": "2021-05-06T14:42:39Z"
    },
    "source": "arxiv",
    "title": "Do You Even Need Attention? A Stack of Feed-Forward Layers Does Surprisingly Well on ImageNet"
  },
  "status": "success",
  "usage": {
    "credits_remaining": 4.95,
    "credits_total": 5,
    "credits_used": 0.05,
    "request_cost": 0.01,
    "request_limit": 100000,
    "requests_remaining": 99997
  }
}"##).unwrap()
}

fn serve(status: u16, body: Value) -> Origin {
    Origin::serving(move |_| Response::new(status, serde_json::to_vec(&body).unwrap()))
}

#[test]
fn wiki_recording_maps_content_licence_date_and_observed_charge() {
    let body = recorded_wiki();
    let origin = serve(200, body.clone());
    let acquisition = DatavilleAdapter::new(&origin.url(), "dataville_test-key")
        .search("Apollo 11 / Moon", 5, &[])
        .unwrap();
    let request = origin.only_request();
    assert_eq!(request.method, "GET");
    assert_eq!(request.target, "/wiki/Apollo%2011%20%2F%20Moon");
    assert_eq!(
        request.header("Authorization"),
        Some("Bearer dataville_test-key")
    );
    assert_eq!(request.header("Accept"), Some("application/json"));
    assert!(request.body.is_empty());
    assert_eq!(
        acquisition.endpoint,
        format!("{}/wiki/Apollo%2011%20%2F%20Moon", origin.url())
    );
    assert_eq!(acquisition.provider, "dataville");
    assert_eq!(acquisition.capability, ProviderCapability::Search);
    assert_eq!(acquisition.http_status, Some(200));
    assert!(acquisition.provider_request_id.is_none());
    assert_eq!(
        serde_json::from_slice::<Value>(&acquisition.raw_response).unwrap(),
        body
    );
    assert_eq!(acquisition.envelopes.len(), 1);
    let item = &acquisition.envelopes[0];
    assert_eq!(item.source_url, "https://en.wikipedia.org/wiki/Apollo_11");
    assert_eq!(item.host, "en.wikipedia.org");
    assert_eq!(item.title.as_deref(), Some("Apollo 11"));
    assert_eq!(item.retrieval_rank, 1);
    assert_eq!(item.text.as_deref(), body["data"]["body"].as_str());
    assert_eq!(
        item.content_hash.as_deref(),
        Some(
            commonmeasure_types::canonical::sha256_digest(item.text.as_ref().unwrap().as_bytes())
                .as_str()
        )
    );
    assert_eq!(
        item.licence,
        LicenceState::Declared {
            reference: "https://creativecommons.org/licenses/by-sa/4.0/".into()
        }
    );
    let date = item.declared_date.as_ref().unwrap();
    assert_eq!(date.date, "2026-08-24T18:22:02.000Z");
    assert!(date.provenance.contains("data.last_updated"));
    assert!(
        date.provenance
            .contains("revision time, not the publication date")
    );
    for field in ["entities", "language", "source", "metadata", "attribution"] {
        assert_eq!(item.native_metadata[field], body["data"][field]);
    }
    for field in ["title", "body", "last_updated"] {
        assert!(item.native_metadata.get(field).is_none());
    }
    assert_eq!(acquisition.charge.money.unwrap().micros, 30_000);
    let native = acquisition.charge.native.unwrap();
    assert_eq!(native.basis, ChargeBasis::Observed);
    assert_eq!(native.unit, "USD");
    assert_eq!(native.amount, serde_json::Number::from_f64(0.03).unwrap());
}

#[test]
fn arxiv_recording_uses_the_abstract_url_even_when_pdf_url_is_null() {
    let body = recorded_arxiv();
    let origin = serve(200, body.clone());
    let acquisition = DatavilleAdapter::new(&origin.url(), "test-key")
        .search(
            "attention is all you need",
            10,
            &["unmapped.example", "arxiv.org", "en.wikipedia.org"],
        )
        .unwrap();
    assert_eq!(
        origin.only_request().target,
        "/arxiv/attention%20is%20all%20you%20need"
    );
    assert_eq!(acquisition.envelopes.len(), 1);
    let item = &acquisition.envelopes[0];
    assert_eq!(item.source_url, "http://arxiv.org/abs/2105.02723v1");
    assert_eq!(item.host, "arxiv.org");
    assert_eq!(item.licence, LicenceState::Unknown);
    assert_eq!(item.retrieval_rank, 1);
    let date = item.declared_date.as_ref().unwrap();
    assert_eq!(date.date, "2021-05-06T14:42:39Z");
    assert!(
        date.provenance
            .contains("data.last_updated on Dataville arxiv")
    );
    assert!(!date.provenance.contains("revision"));
    assert_eq!(item.native_metadata["metadata"], body["data"]["metadata"]);
    assert_eq!(acquisition.charge.money.unwrap().micros, 10_000);
}

#[test]
fn host_order_selects_one_source_without_fan_out() {
    let origin = serve(200, recorded_wiki());
    DatavilleAdapter::new(&origin.url(), "test-key")
        .search("q", 100, &["en.wikipedia.org", "arxiv.org"])
        .unwrap();
    assert_eq!(origin.only_request().target, "/wiki/q");
}

#[test]
fn unmapped_hosts_are_refused_without_a_request() {
    let origin = serve(200, recorded_wiki());
    let error = DatavilleAdapter::new(&origin.url(), "test-key")
        .search("q", 2, &["example.org", "arxiv.org.attacker.example"])
        .unwrap_err();
    let detail = error.to_string();
    assert!(detail.contains("serves none"));
    assert!(detail.contains("example.org"));
    assert!(detail.contains("arxiv.org.attacker.example"));
    assert!(origin.requests().is_empty());
}

#[test]
fn registration_declares_only_search_with_a_one_result_cap() {
    let adapter = remote_adapter("dataville", Some("http://127.0.0.1:1"), "test-key").unwrap();
    assert_eq!(required_variable("dataville"), Some("DATAVILLE_API_KEY"));
    assert!(declared_provider_ref("dataville").is_some());
    assert_eq!(adapter.capabilities(), &[ProviderCapability::Search]);
    assert_eq!(adapter.maximum_search_results(), Some(1));
    assert!(matches!(
        adapter.fetch("https://en.wikipedia.org/wiki/Apollo_11"),
        Err(SupplyError::CapabilityUnavailable {
            capability: ProviderCapability::Fetch,
            ..
        })
    ));
}

#[test]
fn anonymous_success_is_a_credential_failure_even_with_a_result() {
    let mut body = recorded_wiki();
    body["account_state"] = json!("anonymous");
    let origin = serve(200, body);
    let error = DatavilleAdapter::new(&origin.url(), "rejected-key")
        .search("q", 1, &[])
        .unwrap_err();
    assert!(
        matches!(&error, SupplyError::Status {status: 200, detail, ..} if detail.contains("rejected") && detail.contains("DATAVILLE_API_KEY"))
    );
    assert!(!error.to_string().contains("rejected-key"));
    origin.only_request();
}

#[test]
fn status_error_on_200_keeps_the_provider_message_and_code() {
    let origin = serve(
        200,
        json!({"status":"error", "account_state":"authenticated", "data":{"error":"source is unavailable", "code":"UPSTREAM_UNAVAILABLE"}}),
    );
    let error = DatavilleAdapter::new(&origin.url(), "test-key")
        .search("q", 1, &[])
        .unwrap_err();
    assert!(
        matches!(error, SupplyError::Status {status: 200, detail, ..} if detail.contains("source is unavailable") && detail.contains("UPSTREAM_UNAVAILABLE"))
    );
    origin.only_request();
}

#[test]
fn non_success_statuses_keep_no_results_and_insufficient_credit_details() {
    for (status, data) in [
        (404, json!({"error":"No results found for zzzzzz in arxiv"})),
        (402, json!({"error":"insufficient_credit"})),
    ] {
        let origin = serve(
            status,
            json!({"status":"error", "account_state":"authenticated", "data":data}),
        );
        let error = DatavilleAdapter::new(&origin.url(), "test-key")
            .search("zzzzzz", 1, &["arxiv.org"])
            .unwrap_err();
        assert!(
            matches!(error, SupplyError::Status {status: seen, detail, ..} if seen == status && detail.contains(data["error"].as_str().unwrap()))
        );
        origin.only_request();
    }
}

#[test]
fn missing_or_mistyped_success_fields_are_not_empty_successes() {
    for (field, replacement) in [
        ("data", None),
        ("data", Some(json!([]))),
        ("account_state", None),
        ("status", None),
    ] {
        let mut body = recorded_wiki();
        if let Some(value) = replacement {
            body[field] = value;
        } else {
            body.as_object_mut().unwrap().remove(field);
        }
        let origin = serve(200, body);
        assert!(matches!(
            DatavilleAdapter::new(&origin.url(), "test-key").search("q", 1, &[]),
            Err(SupplyError::Malformed { .. })
        ));
    }
}

#[test]
fn absent_url_drops_the_envelope_and_empty_fields_stay_unknown() {
    for url in [
        Value::Null,
        json!("not a URL"),
        json!("https://en.wikipedia.org/wiki/Apollo_11"),
    ] {
        let mut body = recorded_wiki();
        body["data"]["metadata"]["url"] = url.clone();
        body["data"]["body"] = json!("");
        body["data"]["title"] = json!("");
        body["data"]["last_updated"] = json!("");
        body["data"]["attribution"]["license"]["url"] = json!("");
        body["usage"]
            .as_object_mut()
            .unwrap()
            .remove("request_cost");
        let origin = serve(200, body);
        let acquisition = DatavilleAdapter::new(&origin.url(), "test-key")
            .search("q", 1, &[])
            .unwrap();
        assert!(acquisition.charge.money.is_none());
        assert!(acquisition.charge.native.is_none());
        if url == "https://en.wikipedia.org/wiki/Apollo_11" {
            let item = &acquisition.envelopes[0];
            assert!(
                item.text.is_none()
                    && item.content_hash.is_none()
                    && item.title.is_none()
                    && item.declared_date.is_none()
            );
            assert_eq!(item.licence, LicenceState::Unknown);
        } else {
            assert!(acquisition.envelopes.is_empty());
        }
    }
}

#[test]
fn a_finer_than_microdollar_charge_retains_the_native_observation() {
    let mut body = recorded_arxiv();
    body["usage"]["request_cost"] = json!(0.0000001);
    let origin = serve(200, body);
    let acquisition = DatavilleAdapter::new(&origin.url(), "test-key")
        .search("q", 1, &["arxiv.org"])
        .unwrap();
    assert!(acquisition.charge.money.is_none());
    let native = acquisition.charge.native.unwrap();
    assert_eq!(native.basis, ChargeBasis::Observed);
    assert!(native.note.unwrap().contains("micro-dollar precision"));
}

#[test]
fn the_documented_stale_response_without_a_url_stays_raw() {
    // The OpenAPI stale example supplies only cache metadata, with no URL.
    let body = json!({"status":"success", "account_state":"authenticated", "data":{
        "id":"2301.00001", "title":"Attention Is All You Need", "body":"Authors: Ashish Vaswani…", "source":"arxiv", "language":"en", "last_updated":"2017-06-12T00:00:00Z", "entities":[],
        "metadata":{"served_from_cache":true, "cached_at":"2026-08-24T09:12:00Z"},
        "stale":{"message":"arxiv is unavailable right now, so this result comes from our stored copy and may be out of date or incomplete. Nothing is wrong with your query.", "cached_at":"2026-08-24T09:12:00Z"}},
        "usage":{"requests_remaining":49, "request_limit":50}});
    let origin = serve(200, body.clone());
    let acquisition = DatavilleAdapter::new(&origin.url(), "test-key")
        .search("q", 1, &["arxiv.org"])
        .unwrap();
    assert!(acquisition.envelopes.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&acquisition.raw_response).unwrap(),
        body
    );
    assert!(acquisition.charge.money.is_none() && acquisition.charge.native.is_none());
}

#[test]
fn a_stale_notice_on_a_record_with_a_url_remains_in_native_metadata() {
    // Fault injection over the recorded shape; this stale response was not live.
    let mut body = recorded_arxiv();
    let notice = json!({"message":"arxiv is unavailable right now, so this result comes from our stored copy and may be out of date or incomplete. Nothing is wrong with your query.", "cached_at":"2026-08-24T09:12:00Z"});
    body["data"]["stale"] = notice.clone();
    let origin = serve(200, body);
    let acquisition = DatavilleAdapter::new(&origin.url(), "test-key")
        .search("q", 1, &["arxiv.org"])
        .unwrap();
    assert_eq!(acquisition.envelopes[0].native_metadata["stale"], notice);
    assert_eq!(
        acquisition.envelopes[0]
            .declared_date
            .as_ref()
            .unwrap()
            .date,
        "2021-05-06T14:42:39Z"
    );
}
