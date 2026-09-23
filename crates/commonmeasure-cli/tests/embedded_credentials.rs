//! Fixture-tested acquisition through the real MCP binary, HTTP transport,
//! C2PA verifier and transform. Signed fixtures use an ephemeral test identity.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use c2pa::assertions::DataHash;
use commonmeasure_http::{Response, Server};
use commonmeasure_types::canonical::sha256_digest;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const TEXT: &str = "The quarterly measurement increased.";
const HTML: &str =
    "<html><head></head><body><p>The quarterly measurement increased.</p></body></html>";

fn signed(text: &str, html: bool) -> (String, Vec<u8>) {
    let signer = c2pa::EphemeralSigner::new("acquisition-test.local").unwrap();
    let context = c2pa::Context::new().with_signer(signer).into_shared();
    let mut length = 0;
    for _ in 0..10 {
        let mut builder = c2pa::Builder::from_shared_context(&context)
            .with_definition(json!({
                "claim_generator_info":[{"name":"acquisition-test"}],
                "format": if html { "text/html" } else { "text/plain" },
                "assertions":[{"label":"c2pa.actions", "data":{"actions":[{
                    "action":"c2pa.created",
                    "digitalSourceType":"http://cv.iptc.org/newscodes/digitalsourcetype/digitalCreation"
                }]}}]
            }).to_string()).unwrap();
        let mut binding = DataHash::new("received text", "sha256");
        let start = if html {
            text.find("</head>").unwrap()
        } else {
            text.len()
        };
        binding.add_exclusion(c2pa::HashRange::new(start as u64, length as u64));
        binding.set_hash(Sha256::digest(text.as_bytes()).to_vec());
        builder
            .add_assertion(c2pa::assertions::labels::DATA_HASH, &binding)
            .unwrap();
        let manifest = builder.sign_embeddable("application/c2pa").unwrap();
        let needed = if html {
            c2pa_text::html::build_html_script(&manifest).len()
        } else {
            c2pa_text::worst_case_wrapper_byte_length(manifest.len())
        };
        if needed == length {
            let embedded = if html {
                c2pa_text::html::embed_html_inline(text, &manifest, "")
                    .unwrap()
                    .html
            } else {
                format!(
                    "{text}{}",
                    c2pa_text::encode_wrapper_padded(&manifest, length).unwrap()
                )
            };
            return (embedded, manifest);
        }
        length = needed;
    }
    panic!("fixture exclusion length did not settle");
}

struct Fetched {
    result: Value,
    transform: Value,
    crossing: Value,
    requests: Vec<String>,
}

fn fetch(body: &[u8], content_type: &str, gzip: bool, refuse: bool) -> Fetched {
    let served = if gzip {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(body).unwrap();
        encoder.finish().unwrap()
    } else {
        body.to_vec()
    };
    let received_hash = sha256_digest(&served);
    let content_type = content_type.to_owned();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let origin = Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(move |request| {
            seen.lock().unwrap().push(request.target.clone());
            match request.target.as_str() {
                "/robots.txt" => Response::text(200, "User-agent: *\nAllow: /\n"),
                "/page" => {
                    let mut response = Response::new(200, served.clone());
                    response.headers.set("Content-Type", &content_type);
                    if gzip {
                        response.headers.set("Content-Encoding", "gzip");
                    }
                    if refuse {
                        response.headers.set("Content-Usage", "ai-use=n");
                    }
                    response
                }
                _ => Response::text(404, "absent"),
            }
        })
        .unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("policy.json"),
        r#"{"policy_mode":"strict","allow_private_hosts":true}"#,
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["mcp", "--host", "claude-code", "--session", "embedded"])
        .env("COMMONMEASURE_HOME", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(
        child.stdin.take().unwrap(),
        "{}",
        json!({"jsonrpc":"2.0", "id":1,
        "method":"tools/call", "params":{"name":"context_fetch", "arguments":{
            "url":format!("http://{}/page", origin.addr())
        }}})
    )
    .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["result"]["isError"], refuse, "{response}");
    let result = if refuse {
        Value::Null
    } else {
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    };
    let record = std::fs::read_to_string(home.path().join("sessions/embedded.ndjson")).unwrap();
    let records: Vec<Value> = record
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let transform_at = records
        .iter()
        .position(|r| {
            r["event"] == "processor_invoked"
                && r["payload"]["processor"]["name"] == "html-text-extractor"
        })
        .unwrap();
    let crossing_at = records
        .iter()
        .position(|r| r["event"].as_str().unwrap().starts_with("crossing_"))
        .unwrap();
    assert!(transform_at < crossing_at);
    let transform = records[transform_at]["payload"].clone();
    let crossing = records[crossing_at]["payload"].clone();
    assert_eq!(transform["processor"]["version"], "2");
    assert_eq!(transform["inputs"][0]["content_hash"], received_hash);
    assert_eq!(crossing["retrieved_hash"], received_hash);
    assert_eq!(
        transform["outputs"][0]["content_hash"],
        crossing["content_hash"]
    );
    if !refuse {
        assert_eq!(
            crossing["content_hash"],
            sha256_digest(result["content"].as_str().unwrap().as_bytes())
        );
    }
    let requests = requests.lock().unwrap().clone();
    Fetched {
        result,
        transform,
        crossing,
        requests,
    }
}

fn check_signed(html: bool, tamper: bool, gzip: bool, refuse: bool) {
    let (mut body, manifest) = signed(if html { HTML } else { TEXT }, html);
    if tamper {
        body = body.replacen("increased", "decreased", 1);
    }
    let fetched = fetch(
        body.as_bytes(),
        if html {
            "text/html; charset=utf-8"
        } else {
            "text/plain"
        },
        gzip,
        refuse,
    );
    let verification = &fetched.transform["detail"]["embedded_credential"];
    assert_eq!(
        verification["state"],
        if tamper { "invalid" } else { "untrusted" },
        "{verification:#}"
    );
    assert_eq!(verification["credential_ref"], sha256_digest(&manifest));
    assert!(verification["manifest_label"].is_string());
    assert_eq!(
        verification["method"],
        if html { "annex-a7-script" } else { "annex-a8" }
    );
    assert_ne!(verification["state"], "trusted");
    assert!(
        !verification["validation_status"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let statuses = verification["validation_status"].as_array().unwrap();
    if tamper {
        assert!(
            statuses
                .iter()
                .any(|s| s["code"] == "assertion.dataHash.mismatch")
        );
    } else {
        assert!(
            statuses
                .iter()
                .all(|s| s["code"] == "signingCredential.untrusted")
        );
    }
    assert_eq!(
        fetched.transform["detail"]["credential_wrapper_removed"],
        !html
    );
    if !refuse {
        let expected = if tamper {
            TEXT.replace("increased", "decreased")
        } else {
            TEXT.to_owned()
        };
        assert_eq!(fetched.result["content"], expected);
        assert_ne!(
            fetched.crossing["content_hash"],
            fetched.crossing["retrieved_hash"]
        );
    } else {
        assert_eq!(fetched.crossing["grounded"], false);
        assert_eq!(
            fetched.crossing["content_hash"],
            sha256_digest(TEXT.as_bytes())
        );
    }
}

#[test]
fn text_verifies_before_the_wrapper_is_removed() {
    check_signed(false, false, false, false);
}
#[test]
fn tampered_text_retains_an_invalid_result_after_removal() {
    check_signed(false, true, false, false);
}
#[test]
fn html_verifies_before_the_script_is_removed() {
    check_signed(true, false, false, false);
}
#[test]
fn tampered_html_retains_an_invalid_result_after_extraction() {
    check_signed(true, true, false, false);
}
#[test]
fn gzip_verifies_the_decoded_asset_and_records_the_coded_hash() {
    check_signed(false, false, true, false);
}
#[test]
fn withheld_content_retains_verification_and_both_hashes() {
    check_signed(false, false, false, true);
}

#[test]
fn malformed_manifest_is_invalid_and_keeps_its_reference() {
    let body = c2pa_text::embed_manifest(TEXT, b"not a manifest");
    let fetched = fetch(body.as_bytes(), "text/plain", false, false);
    let verification = &fetched.transform["detail"]["embedded_credential"];
    assert_eq!(verification["state"], "invalid");
    assert_eq!(
        verification["credential_ref"],
        sha256_digest(b"not a manifest")
    );
    assert_eq!(fetched.result["content"], TEXT);
}

#[test]
fn unsupported_media_type_never_acquires_a_trusted_result() {
    let (body, _) = signed(TEXT, false);
    let fetched = fetch(body.as_bytes(), "application/octet-stream", false, false);
    assert_eq!(
        fetched.transform["detail"]["embedded_credential"]["state"],
        "unavailable"
    );
    assert!(!fetched.transform["gaps"].as_array().unwrap().is_empty());
    assert_eq!(fetched.result["content"], body);
}

#[test]
fn unsupported_wrapper_version_is_unavailable_and_preserved() {
    let wrapper = c2pa_text::encode_wrapper(b"not a manifest");
    // U+FE01 is the encoded version byte; this fixture requests version 2.
    let body = format!("{TEXT}{}", wrapper.replacen('\u{fe01}', "\u{fe02}", 1));
    let fetched = fetch(body.as_bytes(), "text/plain", false, false);
    assert_eq!(
        fetched.transform["detail"]["embedded_credential"]["state"],
        "unavailable"
    );
    assert_eq!(fetched.result["content"], body);
}

#[test]
fn external_manifest_is_recorded_as_unavailable_without_a_request() {
    let html = c2pa_text::html::embed_html_reference(HTML, "/credential", "").unwrap();
    let fetched = fetch(html.as_bytes(), "text/html", false, false);
    let verification = &fetched.transform["detail"]["embedded_credential"];
    assert_eq!(verification["state"], "unavailable");
    assert_eq!(verification["credential_ref"], "/credential");
    assert!(!fetched.requests.iter().any(|path| path == "/credential"));
    assert_eq!(fetched.result["content"], TEXT);
}

#[test]
fn unsigned_text_is_absent_and_keeps_equal_hashes() {
    let fetched = fetch(TEXT.as_bytes(), "text/plain", false, false);
    assert_eq!(
        fetched.transform["detail"]["embedded_credential"]["state"],
        "absent"
    );
    assert_eq!(
        fetched.crossing["content_hash"],
        fetched.crossing["retrieved_hash"]
    );
}

#[test]
fn multiple_text_wrappers_are_invalid_and_not_silently_stripped() {
    let (body, _) = signed(TEXT, false);
    let body = format!("{body}{body}");
    let fetched = fetch(body.as_bytes(), "text/plain", false, false);
    assert_eq!(
        fetched.transform["detail"]["embedded_credential"]["state"],
        "invalid"
    );
    assert_eq!(fetched.result["content"], body);
}

#[test]
fn malformed_html_manifest_remains_invalid_after_extraction() {
    let body = HTML.replace(
        "</head>",
        "<script type=\"application/c2pa\">?!</script></head>",
    );
    let fetched = fetch(body.as_bytes(), "text/html", false, false);
    assert_eq!(
        fetched.transform["detail"]["embedded_credential"]["state"],
        "invalid"
    );
    assert_eq!(fetched.result["content"], TEXT);
}

#[test]
fn lossy_decoding_cannot_acquire_a_valid_signature_result() {
    let (body, _) = signed(TEXT, false);
    let mut bytes = body.into_bytes();
    bytes[0] = 0xff;
    let fetched = fetch(&bytes, "text/plain", false, false);
    assert_eq!(
        fetched.transform["detail"]["embedded_credential"]["state"],
        "unavailable"
    );
    assert!(fetched.transform["detail"]["embedded_credential"]["credential_ref"].is_null());
    assert_eq!(fetched.transform["detail"]["lossy_decoding"], true);
    assert_eq!(
        fetched.transform["detail"]["credential_wrapper_removed"],
        false
    );
}
