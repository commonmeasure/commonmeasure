//! The provenance line every `context_fetch` and `context_search` result
//! opens with (`docs/contracts/host-integration.md` §1), driven as a host
//! drives the server: JSON-RPC over stdio to the shipped binary, against
//! loopback origins serving fixed bytes, with the operator's policy in force.
//! Nothing between the request and the line is substituted.

mod common;
use common::{call, crossings, payload, write_policy};

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use commonmeasure_harness::provenance;
use commonmeasure_http::{Response, Server, ServerHandle};
use serde_json::{Value, json};

/// A name the ruling reads as public, resolved to loopback by the debug
/// binary's `COMMONMEASURE_TEST_HOSTS`.
const PUBLIC_NAME: &str = "publisher.test";

/// The byte target for one fetch line (EDG-164), held for the line's own
/// text: every byte but the host name's, which the source chose and which
/// is cut at 64 characters. The issue's own example line is 168 bytes, 11
/// of them its host; each ` · ` separator is four bytes of UTF-8. Each line
/// these tests read prints its whole cost.
const FETCH_LINE_TARGET: usize = 160;

/// An origin with the given `robots.txt` and licence, serving `page` at
/// every other path and no discovery manifest.
fn site(robots: &'static str, licence: &'static str, page: &'static str) -> ServerHandle {
    Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |request| match request.target.as_str() {
            "/robots.txt" => Response::text(200, robots),
            "/license.xml" => Response::new(200, licence.as_bytes().to_vec()),
            "/.well-known/content-telemetry.json" => Response::text(404, "none"),
            _ => Response::text(200, page),
        })
        .expect("spawn")
}

fn article(site: &ServerHandle) -> String {
    format!("{}/article", site.url().replace("127.0.0.1", PUBLIC_NAME))
}

fn converse(home: &Path, corpus: Option<&Path>, requests: &[Value]) -> Vec<Value> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_commonmeasure"));
    command
        .args(["mcp", "--host", "claude-code", "--session", "test-session"])
        .env("COMMONMEASURE_HOME", home)
        .env(
            "COMMONMEASURE_TEST_HOSTS",
            format!("{PUBLIC_NAME}=127.0.0.1"),
        );
    match corpus {
        Some(root) => command.env("COMMONMEASURE_INTERNAL_CORPUS", root),
        None => command.env_remove("COMMONMEASURE_INTERNAL_CORPUS"),
    };
    let mut child = command
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

/// One fetch of `url` under `policy`, answered.
fn fetch(policy: &str, url: &str) -> (tempfile::TempDir, Value) {
    let home = tempfile::tempdir().expect("home");
    write_policy(home.path(), policy);
    let mut answers = converse(
        home.path(),
        None,
        &[call("context_fetch", json!({"url": url}))],
    );
    (home, answers.remove(0))
}

/// The one line a fetch answer opens with, checked against the fixed shape
/// and the byte target.
fn fetch_line(answer: &Value) -> String {
    let lines = provenance::lines(&answer["result"]);
    assert_eq!(lines.len(), 1, "one line per fetch: {answer}");
    let line = lines[0].to_owned();
    let own = line.len() - PUBLIC_NAME.len();
    assert!(
        own < FETCH_LINE_TARGET,
        "{own} bytes besides the host: {line}"
    );
    eprintln!(
        "fetch line, {} bytes, {own} besides the host: {line}",
        line.len()
    );
    line
}

/// The `sha256:` prefix the line shows for a recorded hash.
fn shown_hash(hash: &str) -> String {
    format!("sha256:{}…", &hash["sha256:".len().."sha256:".len() + 8])
}

const SIGNAL_YES: &str = "User-agent: *\nContent-Signal: ai-input=yes\nAllow: /\n";

/// A delivered fetch: the source's preference by the mechanism that stated
/// it, the ruling under the policy mode, the cost unknown (nothing quoted
/// one) and the prefix of the hash the record holds. The payload follows
/// in its own block, unchanged.
#[test]
fn a_delivered_fetch_opens_with_its_line() {
    let site = site(SIGNAL_YES, "", "the delivered article");
    let (home, answer) = fetch(
        r#"{"policy_mode":"strict","allow_private_hosts":true}"#,
        &article(&site),
    );
    assert_eq!(answer["result"]["isError"], false, "{answer}");
    let delivered = payload(&answer);
    assert_eq!(delivered["content"], "the delivered article");
    let hash = delivered["content_hash"].as_str().expect("hash");
    assert_eq!(
        fetch_line(&answer),
        format!(
            "Common Measure · host {PUBLIC_NAME} · terms Content-Signal: ai-input · ruling \
             delivered (strict) · cost unknown · grade mediated · receipt none · {}",
            shown_hash(hash)
        )
    );
    let recorded = crossings(home.path());
    assert_eq!(
        recorded.last().expect("crossing")["payload"]["content_hash"],
        hash
    );
}

/// A fetch where nothing quoted a price and nothing declared terms: every
/// field the record does not hold reads `unknown`, never blank or zero.
#[test]
fn an_unknown_cost_reads_unknown() {
    let site = site("User-agent: *\nAllow: /\n", "", "an unlicensed page");
    let (_home, answer) = fetch(
        r#"{"policy_mode":"observe","allow_private_hosts":true}"#,
        &article(&site),
    );
    assert_eq!(answer["result"]["isError"], false, "{answer}");
    let line = fetch_line(&answer);
    assert!(line.contains(" · terms unknown · "), "{line}");
    assert!(line.contains(" · cost unknown · "), "{line}");
    assert!(!line.contains("cost 0"), "{line}");
}

/// A refused fetch keeps its existing wording in the payload and gains the
/// fields the record holds. Refused before the request, there is no hash.
#[test]
fn a_refused_fetch_keeps_its_wording_and_gains_the_line() {
    let site = site(
        "User-agent: *\nContent-Signal: ai-input=no\nAllow: /\n",
        "",
        "never read",
    );
    let (home, answer) = fetch(
        r#"{"policy_mode":"observe","allow_private_hosts":true}"#,
        &article(&site),
    );
    assert_eq!(answer["result"]["isError"], true, "{answer}");
    let told = payload(&answer)["error"]
        .as_str()
        .expect("the refusal")
        .to_owned();
    let recorded = crossings(home.path());
    let crossing = recorded.last().expect("crossing");
    assert_eq!(crossing["event"], "crossing_refused");
    assert_eq!(
        crossing["payload"]["told"],
        told.as_str(),
        "the wording is the recorded one"
    );
    assert_eq!(
        fetch_line(&answer),
        format!(
            "Common Measure · host {PUBLIC_NAME} · terms Content-Signal: no ai-input · ruling \
             refused (observe) · cost unknown · grade mediated · receipt none · sha256 unknown"
        )
    );
}

/// A licence that quotes a price: the line names the price as quoted, which
/// no settlement rail pays here, so the payment term refuses the fetch.
#[test]
fn a_priced_licence_names_its_quoted_price() {
    const PRICED: &str = r#"<rsl xmlns="https://rslstandard.org/rsl">
  <content url="/"><license>
    <permits type="usage">ai-input</permits>
    <payment type="use"><amount currency="USD">0.015</amount></payment>
  </license></content></rsl>"#;
    let site = site(
        "License: /license.xml\nUser-agent: *\nAllow: /\n",
        PRICED,
        "the priced article",
    );
    let (_home, answer) = fetch(
        r#"{"policy_mode":"strict","allow_private_hosts":true}"#,
        &article(&site),
    );
    assert_eq!(answer["result"]["isError"], true, "{answer}");
    assert!(
        payload(&answer)["error"]
            .as_str()
            .expect("the refusal")
            .contains("payment term is unmet"),
        "{answer}"
    );
    assert_eq!(
        fetch_line(&answer),
        format!(
            "Common Measure · host {PUBLIC_NAME} · terms RSL: ai-input, use · ruling refused \
             (strict) · cost 0.015 USD quoted · grade mediated · receipt none · sha256 unknown"
        )
    );
}

/// A search result names its supplier in place of the ruling. The internal
/// corpus is a supplier whose results carry no host and a licence the
/// operator declared; its hash is the supplier's.
#[test]
fn a_supplier_result_names_its_supplier() {
    let corpus = tempfile::tempdir().expect("corpus");
    std::fs::write(
        corpus.path().join("corpus.json"),
        r#"{"name": "provenance corpus",
            "licence": {"state": "declared", "reference": "test/kb-licence-v1"}}"#,
    )
    .expect("manifest");
    std::fs::write(
        corpus.path().join("guide.md"),
        "# Gateway guide\n\nTune the gateway batch window for throughput.\n",
    )
    .expect("document");
    let canonical = corpus.path().canonicalize().expect("canonical root");
    let home = tempfile::tempdir().expect("home");
    write_policy(
        home.path(),
        &json!({
            "policy_mode": "observe",
            "record_internal_prefixes": [format!("file://{}/", canonical.display())],
        })
        .to_string(),
    );
    let answers = converse(
        home.path(),
        Some(corpus.path()),
        &[call(
            "context_search",
            json!({"query": "gateway batch window", "provider": "internal"}),
        )],
    );
    let answer = &answers[0];
    assert_eq!(answer["result"]["isError"], false, "{answer}");
    let delivered = payload(answer);
    assert_eq!(delivered["provider"], "internal");
    let results = delivered["results"].as_array().expect("results");
    let lines = provenance::lines(&answer["result"]);
    assert_eq!(lines.len(), results.len(), "one line per result: {answer}");
    let hash = results[0]["content_hash"].as_str().expect("hash");
    assert_eq!(
        lines[0],
        format!(
            "Common Measure · host unknown · terms declared by supplier · supplier internal · \
             cost unknown · grade mediated · receipt unknown · {}",
            shown_hash(hash)
        )
    );
}

/// A call that records no crossing has no line, and its payload is the
/// result's only text block.
#[test]
fn a_call_that_records_no_crossing_has_no_line() {
    let home = tempfile::tempdir().expect("home");
    write_policy(home.path(), r#"{"policy_mode":"observe"}"#);
    let answers = converse(
        home.path(),
        None,
        &[
            call("context_status", json!({})),
            call("context_fetch", json!({})),
        ],
    );
    for answer in &answers {
        assert!(provenance::lines(&answer["result"]).is_empty(), "{answer}");
        assert_eq!(
            answer["result"]["content"]
                .as_array()
                .expect("content")
                .len(),
            1,
            "{answer}"
        );
    }
}

/// A refusal at a host a redirect chose names no host: the agent's error
/// names the hop by position alone (EDG-116, EDG-129), and the line holds to
/// the same rule, while the record keeps the hop's URL whole.
#[test]
fn a_refusal_at_a_redirect_host_withholds_it() {
    let destination = site(
        "User-agent: *\nContent-Signal: ai-input=no\nAllow: /\n",
        "",
        "never read",
    );
    let elsewhere = format!(
        "{}/article",
        destination.url().replace("127.0.0.1", "institution.test")
    );
    let start = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |request| match request.target.as_str() {
            "/robots.txt" => Response::text(200, "User-agent: *\nAllow: /\n"),
            "/.well-known/content-telemetry.json" => Response::text(404, "none"),
            _ => {
                let mut response = Response::new(302, Vec::new());
                response.headers.set("Location", &elsewhere);
                response
            }
        })
        .expect("spawn");
    let home = tempfile::tempdir().expect("home");
    write_policy(
        home.path(),
        r#"{"policy_mode":"observe","allow_private_hosts":true}"#,
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_commonmeasure"));
    command
        .args(["mcp", "--host", "claude-code", "--session", "test-session"])
        .env("COMMONMEASURE_HOME", home.path())
        .env(
            "COMMONMEASURE_TEST_HOSTS",
            format!("{PUBLIC_NAME}=127.0.0.1,institution.test=127.0.0.1"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().expect("the binary should start");
    writeln!(
        child.stdin.as_mut().expect("stdin"),
        "{}",
        call("context_fetch", json!({"url": article(&start)}))
    )
    .expect("write request");
    let output = child.wait_with_output().expect("wait");
    let answer: Value =
        serde_json::from_str(String::from_utf8_lossy(&output.stdout).trim()).expect("JSON");
    assert_eq!(answer["result"]["isError"], true, "{answer}");
    let recorded = crossings(home.path());
    let crossing = &recorded.last().expect("crossing")["payload"];
    assert_eq!(crossing["host_name"], "institution.test", "{crossing}");
    let line = fetch_line(&answer);
    assert!(
        line.starts_with("Common Measure · host withheld · "),
        "{line}"
    );
    assert!(!answer.to_string().contains("institution.test"), "{answer}");
}
