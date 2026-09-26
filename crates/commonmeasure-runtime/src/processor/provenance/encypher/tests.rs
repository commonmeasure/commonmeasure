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

/// A credential Encypher could return, signed over `sources` as the request
/// named them.
fn signed_response(case: &str, sources: &[Source]) -> String {
    let policy = policy();
    let signing = SigningIdentity::Unconfigured;
    let input = input(&policy, sources, &signing);
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
    run_with(case, sources(), &sources())
}

/// Run the adapter on `sources` against a fixture that returns a credential
/// signed over `sent`, the sources the request is expected to name.
fn run_with(
    case: &'static str,
    sources: Vec<Source>,
    sent: &[Source],
) -> (Value, Option<Label>, Vec<Request>) {
    let signed = signed_response(case, sent);
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

/// Sources with each reference in `references`, and the same sources with
/// each reference replaced by its counterpart in `sent`.
fn sources_sent_as(references: &[(&str, &str)]) -> (Vec<Source>, Vec<Source>) {
    let recorded: Vec<Source> = references
        .iter()
        .enumerate()
        .map(|(index, (reference, _))| Source {
            reference: (*reference).to_owned(),
            content_hash: sha256_digest(format!("source {index}").as_bytes()),
            grade: Grade::Mediated,
        })
        .collect();
    let sent = recorded
        .iter()
        .zip(references)
        .map(|(source, (_, sent))| Source {
            reference: (*sent).to_owned(),
            ..source.clone()
        })
        .collect();
    (recorded, sent)
}

/// Every reference the request names: the ingredient titles and the source
/// record's sources, in that order.
fn sent_references(request: &Request) -> Vec<String> {
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    let assertions = body["options"]["custom_assertions"].as_array().unwrap();
    let mut references: Vec<String> = assertions[2..]
        .iter()
        .map(|assertion| assertion["data"]["dc:title"].as_str().unwrap().to_owned())
        .collect();
    references.extend(
        assertions[0]["data"]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|source| source["reference"].as_str().unwrap().to_owned()),
    );
    references
}

/// Checks the exchange admits, that the request names `expected` twice over
/// (ingredients, then source record) and that the run's own record keeps the
/// references as recorded. Returns the serialised request body.
fn exchange_sends(references: &[(&str, &str)]) -> String {
    let (recorded, sent) = sources_sent_as(references);
    let (invocation, label, requests) = run_with("success", recorded.clone(), &sent);
    assert_eq!(requests.len(), 1);
    let expected: Vec<String> = references
        .iter()
        .chain(references)
        .map(|(_, sent)| (*sent).to_owned())
        .collect();
    assert_eq!(sent_references(&requests[0]), expected);
    assert_eq!(invocation["decision"], "admit", "{invocation:#}");
    let read = read_back(&label.unwrap().text).unwrap();
    assert_eq!(read.ingredients, sent, "the label names what was sent");
    let inputs: Vec<&str> = invocation["inputs"].as_array().unwrap()[1..]
        .iter()
        .map(|input| input["reference"].as_str().unwrap())
        .collect();
    let kept: Vec<&str> = recorded.iter().map(|s| s.reference.as_str()).collect();
    assert_eq!(inputs, kept, "the local record keeps each reference");
    let definition = &invocation["detail"]["manifest_definition"]["ingredients"];
    for (ingredient, source) in definition.as_array().unwrap().iter().zip(&recorded) {
        assert_eq!(ingredient["title"], source.reference.as_str());
    }
    String::from_utf8(requests[0].body.clone()).unwrap()
}

#[test]
fn credentials_in_manifest_and_supplier_urls_do_not_reach_encypher() {
    let body = exchange_sends(&[
        (
            "https://u:p@host.example/m.json",
            "https://host.example/m.json",
        ),
        (
            "https://acct-7:supplier-secret@supplier.example/items/7?edition=2",
            "https://supplier.example/items/7?edition=2",
        ),
    ]);
    for leaked in ["u:p", "acct-7", "supplier-secret", "@"] {
        assert!(!body.contains(leaked), "the request carries {leaked:?}");
    }
}

#[test]
fn credentials_in_an_unparseable_reference_do_not_reach_encypher() {
    let references = [
        // An out-of-range port and a space in an opaque host fail to parse.
        (
            "https://user:pw@host.example:99999/m.json",
            "https://host.example:99999/m.json",
        ),
        (
            "terms://user:pw@legal desk/contract-7",
            "terms://legal desk/contract-7",
        ),
    ];
    for (reference, _) in references {
        assert!(url::Url::parse(reference).is_err(), "{reference} parses");
    }
    let body = exchange_sends(&references);
    for leaked in ["user", "pw", "@"] {
        assert!(!body.contains(leaked), "the request carries {leaked:?}");
    }
}

#[test]
fn only_the_userinfo_of_a_sent_reference_changes() {
    exchange_sends(&[
        // No case folding, default-port removal or path resolution.
        (
            "HTTPS://U:P@Host.Example:443/a/../m.json",
            "HTTPS://Host.Example:443/a/../m.json",
        ),
        // Without userinfo, byte for byte, however a parser would reserialise
        // it and wherever an `@` sits outside the authority.
        (
            "HTTPS://Host.Example:443/a/../b?q=c@d#f@g",
            "HTTPS://Host.Example:443/a/../b?q=c@d#f@g",
        ),
        ("https://host.example/u@b", "https://host.example/u@b"),
        (r"http://h\@evil.example/l", r"http://h\@evil.example/l"),
        (
            "mailto:licensing@publisher.example",
            "mailto:licensing@publisher.example",
        ),
        ("plans/local/source@1", "plans/local/source@1"),
        ("urn:example:source:7", "urn:example:source:7"),
    ]);
}

#[test]
fn a_reference_whose_userinfo_cannot_be_cut_from_the_text_is_sent_as_serialised() {
    // Without slashes after the scheme there is no authority to cut from the
    // text, but the parser still reads `u:p` as userinfo.
    let body = exchange_sends(&[("https:u:p@h.example/f", "https://h.example/f")]);
    for leaked in ["u:p", "@"] {
        assert!(!body.contains(leaked), "the request carries {leaked:?}");
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
