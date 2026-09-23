//! These controlled HTTP fixtures exercise the adapter, not Encypher's service.

use super::*;
use commonmeasure_http::{Response, Server};
use std::sync::{Arc, Mutex};

const ANSWER: &str = "A synthetic answer about cafe\u{301} supplies.";
const KEY: &str = "fixture-api-key-do-not-record";
const CHAIN: &str = include_str!("../../../../../../demo/provenance/signer.pem");
const PRIVATE_KEY: &[u8] = include_bytes!("../../../../../../demo/provenance/signer-key.pem");

fn policy() -> OutputProvenance {
    OutputProvenance {
        training_mining: TrainingMining {
            ai_inference: Use::Allowed,
            ai_training: Use::NotAllowed,
            ai_generative_training: Use::NotAllowed,
            data_mining: Use::NotAllowed,
            constraint_info: None,
        },
        encypher: Some(Disclosure {
            send_answer_and_source_record: true,
        }),
    }
}

fn sources() -> Vec<Source> {
    vec![
        Source {
            reference: "https://source.example/first".to_owned(),
            content_hash: sha256_digest(b"first source"),
            grade: Grade::Mediated,
        },
        Source {
            reference: "https://source.example/second".to_owned(),
            content_hash: sha256_digest(b"second source"),
            grade: Grade::Observed,
        },
    ]
}

fn input<'a>(
    policy: &'a OutputProvenance,
    sources: &'a [Source],
    signing: &'a SigningIdentity,
) -> Input<'a> {
    Input {
        run_id: "synthetic-run",
        plan_id: "synthetic-plan",
        run_manifest_hash: "sha256:synthetic",
        answer: ANSWER,
        answer_reference: "plans/synthetic-plan/answer",
        sources,
        policy,
        signing,
        labelled_reference: "provenance/synthetic-plan.txt",
        manifest_reference: "provenance/synthetic-plan.c2pa",
    }
}

fn signed_response(case: &str) -> String {
    let policy = policy();
    let sources = sources();
    let signing = SigningIdentity::Unconfigured;
    let input = input(&policy, &sources, &signing);
    let mut definition = definition(&input);
    match case {
        "source_record" => definition["assertions"][2]["data"]["run_id"] = json!("another-run"),
        "training" => {
            definition["assertions"][1]["data"]["entries"]["cawg.ai_training"]["use"] =
                json!("allowed")
        }
        "missing_training" => {
            definition["assertions"].as_array_mut().unwrap().remove(1);
        }
        "ingredient" => {
            definition["ingredients"][0]["instance_id"] = json!(sha256_digest(b"other source"))
        }
        "grade" => definition["ingredients"][0]["description"] = json!("reconstructed"),
        "relationship" => definition["ingredients"][0]["relationship"] = json!("componentOf"),
        "action" => {
            definition["assertions"][0]["data"]["actions"][0]["digitalSourceType"] =
                json!("http://cv.iptc.org/newscodes/digitalsourcetype/digitalCreation")
        }
        "extra_ingredient" => {
            let extra = definition["ingredients"][0].clone();
            definition["ingredients"]
                .as_array_mut()
                .unwrap()
                .push(extra);
        }
        "duplicate_assertion" => {
            let mut extra = definition["assertions"][2].clone();
            extra["data"]["run_id"] = json!("conflicting-run");
            definition["assertions"].as_array_mut().unwrap().push(extra);
        }
        "duplicate_actions" => {
            let mut extra = definition["assertions"][0].clone();
            extra["data"]["actions"][0]["action"] = json!("c2pa.edited");
            definition["assertions"].as_array_mut().unwrap().push(extra);
        }
        "reflected_key" => definition["assertions"]
            .as_array_mut()
            .unwrap()
            .push(json!({"label": "com.example.fixture", "data": {"reflected": KEY}})),
        "reflected_label" => {
            definition["label"] = json!(format!(
                "urn:c2pa:550e8400-e29b-41d4-a716-446655440000:{KEY}"
            ));
        }
        _ => {}
    }
    let material = SigningMaterial {
        certificate_chain: CHAIN.as_bytes().to_vec(),
        private_key: PRIVATE_KEY.to_vec(),
        certificate_ref: "fixture".to_owned(),
        key_ref: "fixture".to_owned(),
    };
    let text = if case == "answer" {
        "An entirely different answer."
    } else {
        ANSWER
    };
    let signed = super::super::sign(&definition, text, &material).unwrap();
    match case {
        "tampered" | "reflected_label" => signed.text.replacen('A', "B", 1),
        "no_label" => ANSWER.to_owned(),
        _ => signed.text,
    }
}

fn run_case(case: &'static str) -> (Value, Option<Label>, Vec<Request>) {
    let signed = signed_response(case);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&seen);
    let server = Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(move |request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            captured.lock().unwrap().push(request.clone());
            if case == "http_failure" {
                return Response::json(
                    401,
                    &json!({"error": {"message": format!("{KEY} {ANSWER}")}}).to_string(),
                );
            }
            if case == "redirect" {
                let mut response = Response::text(307, "");
                response.headers.set("Location", "http://127.0.0.1:1/steal");
                return response;
            }
            if case == "malformed" {
                return Response::json(201, "{");
            }
            let mut document = json!({"document_id": body["document_id"], "signed_text": signed});
            if case == "coerced" {
                document["coerced_options"] = json!({"store_c2pa_manifest": true});
            }
            if case == "wrong_document" {
                document["document_id"] = json!("another-document");
            }
            Response::json(
                201,
                &json!({"success": case != "failed_envelope", "data": {"document": document}})
                    .to_string(),
            )
        })
        .unwrap();
    let root = &CHAIN[CHAIN.rfind("-----BEGIN CERTIFICATE-----").unwrap()..];
    let trust = if case == "untrusted" {
        CHAIN
            [..CHAIN.find("-----END CERTIFICATE-----").unwrap() + "-----END CERTIFICATE-----".len()]
            .to_owned()
    } else {
        root.to_owned()
    };
    let capture = tempfile::tempdir().unwrap();
    let signing = SigningIdentity::Encypher(SigningConfig {
        api_key: KEY.to_owned(),
        trust_anchors: trust,
        endpoint: server.url(),
        capture_directory: Some(capture.path().to_path_buf()),
    });
    let mut policy = policy();
    if case == "no_consent" {
        policy.encypher = None;
    }
    if case == "denied_consent" {
        policy
            .encypher
            .as_mut()
            .unwrap()
            .send_answer_and_source_record = false;
    }
    let unconfigured = SigningIdentity::Unconfigured;
    let signing = if case == "missing_config" {
        &unconfigured
    } else {
        &signing
    };
    let sources = sources();
    let (invocation, label) = super::super::invoke(&input(&policy, &sources, signing));
    assert_eq!(
        capture.path().join("response.json").exists(),
        label.is_some(),
        "{case}"
    );
    if label.is_some() {
        let served = std::fs::read(capture.path().join("response-served.bin")).unwrap();
        assert_eq!(
            invocation.to_value()["detail"]["remote"]["response_hash"],
            sha256_digest(&served)
        );
    }
    let requests = seen.lock().unwrap().clone();
    (invocation.to_value(), label, requests)
}

#[test]
fn full_credential_is_independently_checked_before_publication() {
    let (invocation, label, requests) = run_case("success");
    assert_eq!(invocation["decision"], "admit", "{invocation:#}");
    assert!(label.is_some());
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].headers.get("Authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(
        requests[0].headers.get("Idempotency-Key"),
        Some(sha256_digest(&requests[0].body).as_str())
    );
    assert_eq!(body["text"], ANSWER);
    assert_eq!(body["options"]["index_for_attribution"], false);
    assert_eq!(body["options"]["store_c2pa_manifest"], false);
    let assertions = body["options"]["custom_assertions"].as_array().unwrap();
    assert_eq!(assertions.len(), 4);
    assert_eq!(assertions[2]["data"]["relationship"], "inputTo");
    assert_eq!(
        assertions[2]["data"]["dc:title"],
        "https://source.example/first"
    );
    assert_eq!(assertions[2]["data"]["dc:format"], "text/plain");
    assert_eq!(assertions[3]["label"], "c2pa.ingredient.v3__1");
    assert_eq!(body["options"]["validate_assertions"], true);
    assert_eq!(assertions[3]["data"]["description"], "observed");
    assert_eq!(invocation["detail"]["validation"]["state"], "trusted");
    assert!(invocation["detail"]["remote"]["cost"].is_null());
    let report = invocation.to_string();
    assert!(!report.contains(KEY));
    assert!(!report.contains(ANSWER));
    assert!(!report.contains("first source"));
    assert!(manifest().permissions.network);
    assert_ne!(
        manifest().configuration_digest,
        super::super::manifest().configuration_digest
    );
}

#[test]
fn unavailable_credentials_or_disclosure_produce_no_request() {
    for case in ["no_consent", "denied_consent", "missing_config"] {
        let (invocation, label, requests) = run_case(case);
        assert_eq!(invocation["decision"], "abstain", "{case}");
        assert!(label.is_none(), "{case}");
        assert!(requests.is_empty(), "{case}");
    }
}

#[test]
fn signed_but_wrong_claims_and_failed_responses_never_publish_a_label() {
    for case in [
        "source_record",
        "training",
        "missing_training",
        "ingredient",
        "grade",
        "relationship",
        "action",
        "extra_ingredient",
        "duplicate_assertion",
        "duplicate_actions",
        "reflected_key",
        "reflected_label",
        "answer",
        "tampered",
        "no_label",
        "untrusted",
        "http_failure",
        "redirect",
        "malformed",
        "coerced",
        "wrong_document",
        "failed_envelope",
    ] {
        let (invocation, label, requests) = run_case(case);
        assert_eq!(invocation["decision"], "abstain", "{case}: {invocation:#}");
        assert!(label.is_none(), "{case}");
        assert_eq!(requests.len(), 1, "{case}: no retry or redirect");
        if case == "missing_training" {
            assert_eq!(invocation["detail"]["validation"]["state"], "trusted");
        }
        assert!(!invocation.to_string().contains(KEY), "{case}");
        assert!(!invocation.to_string().contains(ANSWER), "{case}");
    }
}

#[test]
fn signing_configuration_debug_never_prints_the_key() {
    let config = SigningConfig {
        api_key: KEY.to_owned(),
        trust_anchors: CHAIN.to_owned(),
        endpoint: ENDPOINT.to_owned(),
        capture_directory: None,
    };
    assert!(!format!("{config:?}").contains(KEY));
}

#[test]
fn synthetic_capture_checks_decoded_json_keys_and_values() {
    let value: Value =
        serde_json::from_str(r#"{"value":"\u0066ixture-api-key-do-not-record"}"#).unwrap();
    assert!(json_contains_key(&value, KEY));
    let quoted_key = "fixture-\"\\key";
    assert!(json_contains_key(
        &json!({"nested": [{quoted_key: true}]}),
        quoted_key
    ));
    assert!(!json_contains_key(&json!({"value": "unrelated"}), KEY));
}

/// Calls the real service with synthetic content only. This establishes signing
/// interoperability, not an acquiring-agent lifecycle or real publisher usage.
#[test]
#[ignore = "requires an authorised Encypher key, trust bundle and explicit output directory"]
fn live_encypher_signs_synthetic_content() {
    let directory = std::env::var("COMMONMEASURE_ENCYPHER_LIVE_OUTPUT")
        .expect("set COMMONMEASURE_ENCYPHER_LIVE_OUTPUT for synthetic trial artefacts");
    let mut config = SigningConfig::from_environment().expect("authorised signing configuration");
    let run_id = format!("synthetic-{}", uuid::Uuid::new_v4());
    let directory = std::path::Path::new(&directory).join(&run_id);
    std::fs::create_dir_all(&directory).unwrap();
    config.capture_directory = Some(directory.clone());
    let signing = SigningIdentity::Encypher(config);
    let policy = policy();
    let sources = sources();
    let mut input = input(&policy, &sources, &signing);
    input.run_id = &run_id;
    let (invocation, label) = super::super::invoke(&input);
    let record = invocation.to_value();
    std::fs::write(
        directory.join("receipt.json"),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();
    assert_eq!(record["decision"], "admit", "{record:#}");
    let label = label.expect("a verified credential");
    std::fs::write(directory.join("output.txt"), label.text).unwrap();
    std::fs::write(directory.join("output.c2pa"), label.manifest).unwrap();
}
