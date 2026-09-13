//! The committed example policy (`demo/policy/four-fetches.json`), the
//! smallest source policy that makes the four fetches of the example
//! (`docs/guide/four-fetches.md`) come out as that page shows: two hosts admitted, the Guardian refused on the payment term its
//! licence states, the Economist refused by the operator's own rule.
//!
//! The policy is read through the runtime's own loader, the host rulings are
//! asserted without a network, and the licence refusal is driven through the
//! real binary against a loopback publisher that stands where the Guardian
//! stands: a `robots.txt` naming an RSL licence whose permit for AI input
//! carries a subscription payment.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use commonmeasure_harness::policy::SessionPolicy;
use commonmeasure_http::{Response, Server, ServerHandle};
use commonmeasure_runtime::policy::Ruling;
use commonmeasure_types::PolicyMode;
use serde_json::{Value, json};

const EXAMPLE_POLICY: &str = include_str!("../../../demo/policy/four-fetches.json");

const ROBOTS: &str = "\
User-agent: *
Allow: /

License: /license.xml
";

/// The shape of the Guardian's licence as the example records it: AI
/// input and training permitted under a subscription, nothing else
/// permitted.
const LICENCE: &str = r#"<rsl xmlns="https://rslstandard.org/rsl">
  <content url="/">
    <license>
      <permits type="usage">ai-train ai-input</permits>
      <payment type="subscription">
        <custom>https://licensing.example.com/</custom>
      </payment>
    </license>
  </content>
</rsl>"#;

/// A publisher on loopback serving the `robots.txt`, the licence and a page.
fn publisher() -> ServerHandle {
    Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(|request| match request.target.as_str() {
            "/robots.txt" => Response::text(200, ROBOTS),
            "/license.xml" => {
                let mut response = Response::new(200, LICENCE.as_bytes().to_vec());
                response.headers.set("Content-Type", "application/rsl+xml");
                response
            }
            _ => Response::text(200, "the article text"),
        })
        .expect("spawn")
}

/// Speak to the server the way a host does: one JSON object per line in,
/// one response per line out.
fn converse(home: &Path, requests: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["mcp", "--host", "claude-code", "--session", "four-fetches"])
        .env("COMMONMEASURE_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the binary should start");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        for request in requests {
            writeln!(stdin, "{request}").expect("write request");
        }
    }
    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success(), "the server should exit cleanly");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
        .collect()
}

fn refusal_reason(ruling: Ruling) -> String {
    match ruling {
        Ruling::Refused { reason, .. } => reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// The policy loads through the loader every session uses, in strict mode,
/// and rules on the four hosts as the example shows. The Guardian is
/// admitted at the host level: its refusal comes later, from its licence.
#[test]
fn the_example_policy_loads_and_rules_on_the_four_hosts() {
    let home = tempfile::tempdir().expect("tempdir");
    std::fs::write(home.path().join("policy.json"), EXAMPLE_POLICY).expect("policy");

    let policy = SessionPolicy::load(home.path(), None).expect("the example policy loads");
    assert_eq!(policy.mode(), PolicyMode::Strict);
    assert_eq!(
        policy.constraints().len(),
        4,
        "four access rules and nothing else"
    );

    for admitted in [
        "https://www.gov.uk/government/organisations",
        "https://people.com/",
        "https://www.theguardian.com/politics/2026/sep/09/mr-congeniality-burnham-advises-badenoch-to-ditch-the-point-scoring-over-defence-spending",
    ] {
        assert!(
            matches!(policy.admit_host(admitted), Ruling::Allowed),
            "{admitted} is admitted by a named rule"
        );
    }
    assert_eq!(
        refusal_reason(policy.admit_host("https://www.economist.com/")),
        "access rule 4 (*) refuses host www.economist.com.",
        "the last rule refuses every host the first three do not name"
    );
}

/// Under the example policy, a publisher whose `robots.txt` names an RSL licence
/// permitting AI input under a subscription is refused before its page is
/// requested, with the reason the example quotes. The loopback publisher
/// takes the Guardian's place, so the policy gains one allow rule for the
/// loopback host ahead of the closing `*` and the switch every loopback
/// test needs to reach a private address; the four committed rules are
/// otherwise as written.
#[test]
fn the_example_policy_refuses_a_subscription_licence_on_its_payment_term() {
    let site = publisher();
    let home = tempfile::tempdir().expect("tempdir");
    let mut policy: Value =
        serde_json::from_str(EXAMPLE_POLICY).expect("the example policy is JSON");
    policy["allow_private_hosts"] = json!(true);
    let rules = policy["constraints"].as_array_mut().expect("rules");
    let closing = rules.pop().expect("the closing rule");
    assert_eq!(closing["host"], "*");
    rules.push(json!({"kind": "access_rule", "host": "127.0.0.1", "action": "allow"}));
    rules.push(closing);
    std::fs::write(home.path().join("policy.json"), policy.to_string()).expect("policy");

    let article = format!("{}/news/1", site.url());
    let responses = converse(
        home.path(),
        &[json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                 "params": {"name": "context_fetch", "arguments": {"url": article}}})],
    );
    let expected = format!(
        "The licence {}/license.xml permits AI input under payment type subscription, and this \
         edge holds no settlement rail, so the payment term is unmet.",
        site.url()
    );
    assert_eq!(responses[0]["result"]["isError"], true);
    let detail = responses[0]["result"]["content"][0]["text"]
        .as_str()
        .expect("an error result carries text");
    assert!(detail.contains(&expected), "{detail}");

    let log =
        std::fs::read_to_string(home.path().join("sessions/four-fetches.ndjson")).expect("log");
    let refused: Vec<Value> = log
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("NDJSON"))
        .filter(|record: &Value| record["event"] == "crossing_refused")
        .collect();
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0]["payload"]["refusal"], expected);
    assert!(
        refused[0]["payload"]["http_status"].is_null(),
        "the page was never requested"
    );
}
