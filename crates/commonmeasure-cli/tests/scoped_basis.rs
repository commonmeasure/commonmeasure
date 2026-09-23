//! Fixture-tested policy enforcement through the real MCP binary and HTTP
//! transport over loopback. These fixtures establish scope and record
//! behaviour, without verifying a publisher's legal entitlement.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use commonmeasure_http::{Response, Server};
use serde_json::{Value, json};

const PAGE: &str = "Scoped basis acceptance content.";

fn assessment(basis: &str, content: &str) -> Value {
    json!({"basis":basis, "applicability":"applicable", "version":"2026-09",
        "claimed_issuer":"Fixture publisher", "authority_evidence":["fixture:reuse-clause"],
        "content":[content], "intended_uses":["ai-input"],
        "reason":"The operator assessed this reference as covering AI input for this content."})
}

/// Exercise a single fetch, including the actual origin requests and the
/// source record. A redirect tests scope again at its destination.
fn run_case(basis: Option<&str>, change: &str, mode: &str, expected: &str, header: bool) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let redirect = change == "redirect";
    let handle = Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(move |request| {
            log.lock().unwrap().push(request.target.clone());
            match request.target.as_str() {
                "/robots.txt" => Response::text(
                    200,
                    if header {
                        "User-agent: *\nAllow: /\n"
                    } else {
                        "User-agent: *\nAllow: /\nContent-Signal: ai-input=no\n"
                    },
                ),
                "/covered" if redirect => {
                    let mut response = Response::new(302, Vec::new());
                    response.headers.set("Location", "/outside");
                    response
                }
                "/.well-known/content-telemetry.json" => Response::text(404, "absent"),
                _ => {
                    let mut response = Response::text(200, PAGE);
                    if header {
                        response.headers.set("Content-Usage", "ai-use=n");
                    }
                    response
                }
            }
        })
        .unwrap();
    let url = format!("http://{}/covered", handle.addr());
    let mut terms =
        json!({"host":"127.0.0.1", "reference":"fixture:basis", "requires_reporting":false});
    if let Some(basis) = basis {
        let mut a = assessment(basis, &url);
        match change {
            "content" => a["content"] = json!([format!("{url}/different")]),
            "query" => a["content"] = json!([format!("{url}?edition=other")]),
            "use" => a["intended_uses"] = json!(["train-ai"]),
            "unresolved" => a["applicability"] = json!("unresolved"),
            "reporting" => terms["requires_reporting"] = json!(true),
            _ => (),
        }
        if basis == "exception" {
            a.as_object_mut().unwrap().remove("version");
            a.as_object_mut().unwrap().remove("claimed_issuer");
            a.as_object_mut().unwrap().remove("authority_evidence");
        }
        terms["assessment"] = a;
    }
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("policy.json"),
        json!({
            "policy_mode":mode, "allow_private_hosts":true, "terms":[terms.clone()]
        })
        .to_string(),
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["mcp", "--host", "claude-code", "--session", "scoped-basis"])
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
        "method":"tools/call", "params":{"name":"context_fetch",
        "arguments":{"url":url,"intended_use":"train-ai"}}})
    )
    .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    let refused = mode == "strict" && (expected != "applied" || change == "reporting");
    assert_eq!(response["result"]["isError"], refused, "{response}");
    let record = std::fs::read_to_string(home.path().join("sessions/scoped-basis.ndjson")).unwrap();
    let crossings: Vec<Value> = record
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|r| {
            r["event"]
                .as_str()
                .is_some_and(|e| e.starts_with("crossing_"))
        })
        .collect();
    assert_eq!(crossings.len(), 1, "{record}");
    let payload = &crossings[0]["payload"];
    let declarations = &payload["declarations"];
    let decision = &declarations["assessment_decision"];
    assert_eq!(decision["applicability"], expected);
    assert_eq!(decision["intended_use"], "ai-input");
    assert_eq!(decision["mode"], mode);
    assert_eq!(decision["rule"], "terms (127.0.0.1) assessment");
    assert!(!decision["reason"].as_str().unwrap().is_empty());
    let outcome = if refused {
        "refused"
    } else if expected == "applied" {
        "allowed"
    } else {
        "allowed_with_breach"
    };
    assert_eq!(decision["outcome"], outcome);
    assert_eq!(declarations["terms"], terms);
    assert_eq!(
        declarations["governing"],
        if expected == "applied" {
            "operator_terms"
        } else {
            "statements"
        }
    );
    assert_eq!(declarations["effective"]["ai-input"], "disallow");
    assert!(!declarations["statements"].as_array().unwrap().is_empty());
    if refused {
        assert_eq!(payload["grounded"], false);
        assert_eq!(
            payload["content_hash"].is_null(),
            !header,
            "a response-stage refusal retains the withheld text's hash"
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains(PAGE));
    } else {
        assert!(
            payload["content_hash"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains(PAGE));
        if expected == "applied" {
            assert_eq!(payload["licence"]["reference"], "fixture:basis");
        } else {
            assert_eq!(payload["licence"]["state"], "unknown");
            assert!(!payload["breach"].is_null());
        }
        let result: Value =
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(result["declarations"]["assessment_decision"], *decision);
    }
    let requests: Vec<String> = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|p| p.as_str() != "/.well-known/content-telemetry.json")
        .cloned()
        .collect();
    let mut expected_requests = vec!["/robots.txt"];
    if !refused || header || redirect {
        expected_requests.push("/covered");
    }
    assert_eq!(requests, expected_requests);
    if redirect {
        assert_eq!(decision["url"], url.replace("/covered", "/outside"));
    } else {
        assert_eq!(decision["url"], url);
    }
}

#[test]
fn matching_agreement_preserves_the_source_declaration_and_operator_assessment() {
    run_case(Some("agreement"), "", "strict", "applied", false);
}

#[test]
fn content_and_query_outside_the_agreement_fall_back_to_each_policy_mode() {
    for mode in ["strict", "observe", "prefer"] {
        for change in ["content", "query"] {
            run_case(Some("agreement"), change, mode, "out_of_scope", false);
        }
    }
}

#[test]
fn training_permission_does_not_cover_fetch_even_when_the_agent_claims_training() {
    for mode in ["strict", "observe", "prefer"] {
        run_case(Some("agreement"), "use", mode, "out_of_scope", false);
    }
}

#[test]
fn public_licence_and_operator_assessed_exception_govern_within_scope() {
    for basis in ["public_licence", "exception"] {
        run_case(Some(basis), "", "strict", "applied", false);
        run_case(Some(basis), "content", "strict", "out_of_scope", false);
    }
}

#[test]
fn unresolved_basis_and_supplier_access_reference_alone_never_imply_reuse_rights() {
    for mode in ["strict", "observe", "prefer"] {
        run_case(Some("agreement"), "unresolved", mode, "unresolved", false);
        run_case(None, "", mode, "unresolved", false);
    }
}

#[test]
fn response_declarations_use_the_same_scope_rule_before_delivery() {
    run_case(Some("agreement"), "", "strict", "applied", true);
    run_case(Some("agreement"), "content", "strict", "out_of_scope", true);
}

#[test]
fn agreement_does_not_follow_a_redirect_outside_its_content_scope() {
    run_case(
        Some("agreement"),
        "redirect",
        "strict",
        "out_of_scope",
        false,
    );
}

#[test]
fn applicable_assessment_keeps_its_reporting_duty() {
    run_case(Some("agreement"), "reporting", "strict", "applied", false);
}
