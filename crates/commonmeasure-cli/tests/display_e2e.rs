//! `context_show` through the shipped stdio server: the recorded signed
//! release (`tests/recorded/signed-release/`) is fetched from a loopback
//! origin through the production transport and the release verifier, then a
//! reply is displayed against it by its acquisition handle or content hash.
//!
//! The recorded release asks for `strict` display. The `labelled` mode is
//! the same release with its display mode changed and signed again under a
//! test key, served and verified the same way.

mod common;
use common::{call, records, write_policy};

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use commonmeasure_http::{Response, Server, ServerHandle};
use commonmeasure_types::canonical::sha256_digest;
use serde_json::{Value, json};

const RECORDED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/recorded/signed-release");
const MEDIA_TYPE: &str = "application/x.signed-release+json";

fn recorded_body() -> Vec<u8> {
    std::fs::read(Path::new(RECORDED).join("body.json")).expect("the recorded body")
}

fn recorded_text() -> String {
    let release: Value = serde_json::from_slice(&recorded_body()).unwrap();
    release["text"].as_str().unwrap().to_owned()
}

/// The recorded release with `change` applied and signed again under a test
/// key, which the sidecar then names.
fn resigned_body(change: impl FnOnce(&mut Value)) -> Vec<u8> {
    use base64::Engine as _;
    use ed25519_dalek::{Signer as _, SigningKey};
    let mut release: Value = serde_json::from_slice(&recorded_body()).unwrap();
    change(&mut release);
    let signed: commonmeasure_types::release::Signed =
        serde_json::from_value(release["sidecar"]["signed"].clone()).unwrap();
    let key = SigningKey::from_bytes(&[9; 32]);
    let engine = base64::engine::general_purpose::STANDARD;
    release["sidecar"]["signature"] = json!(
        engine.encode(
            key.sign(&commonmeasure_types::release::signed_message(&signed))
                .to_bytes()
        )
    );
    release["sidecar"]["signer_public_key"] = json!(engine.encode(key.verifying_key().as_bytes()));
    serde_json::to_vec_pretty(&release).unwrap()
}

/// A loopback origin: `/article` is `body` as a signed release, `/page` a
/// plain text page.
fn origin(body: Vec<u8>) -> ServerHandle {
    Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |request| {
            if request.target == "/robots.txt" {
                return Response::text(200, "User-agent: *\nAllow: /\n");
            }
            if request.target.starts_with("/.well-known/") {
                return Response::text(404, "not here");
            }
            if request.target == "/page" {
                return Response::text(200, "A plain page about the harbour bridge.");
            }
            let mut response = Response::new(200, body.clone());
            response.headers.set("Content-Type", MEDIA_TYPE);
            response
        })
        .expect("spawn")
}

/// The stdio server, asked one request at a time so that a later request
/// can name what an earlier answer returned.
struct Edge {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Edge {
    fn start(home: &Path, mode: &str, extra: &[&str]) -> Self {
        write_policy(
            home,
            &format!(r#"{{"policy_mode":"{mode}","allow_private_hosts":true}}"#),
        );
        let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
            .args(["mcp", "--host", "pi", "--session", "test-session"])
            .args(extra)
            .env("COMMONMEASURE_HOME", home)
            .env_remove("AGENT_SESSION_ID")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the binary should start");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let mut edge = Self {
            child,
            stdin,
            stdout,
        };
        edge.ask(json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
                        "params": {"protocolVersion": "2025-06-18",
                                   "clientInfo": {"name": "test", "version": "1"}}}));
        edge
    }

    fn ask(&mut self, request: Value) -> Value {
        writeln!(self.stdin, "{request}").expect("write request");
        self.stdin.flush().expect("flush");
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read answer");
        serde_json::from_str(&line).expect("one JSON object per line")
    }

    fn fetch(&mut self, url: &str) -> Value {
        let answer = self.ask(call("context_fetch", json!({"url": url})));
        assert_eq!(answer["result"]["isError"], false, "{answer}");
        payload(&answer)
    }

    fn show(&mut self, release: &str, items: Value) -> Value {
        self.ask(call(
            "context_show",
            json!({"release": release, "items": items}),
        ))
    }
}

impl Drop for Edge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The JSON payload: the first text block that is not provenance lines.
fn payload(answer: &Value) -> Value {
    let text = commonmeasure_harness::provenance::payload_text(&answer["result"])
        .expect("a tool result carries text");
    serde_json::from_str(text).expect("the payload is JSON")
}

fn error(answer: &Value) -> String {
    assert_eq!(answer["result"]["isError"], true, "{answer}");
    payload(answer)["error"].as_str().unwrap().to_owned()
}

// The reply's own words, recognisable so that a test can say where they
// appear. Each sentence reaches one row of the display rule.
const CONFIRMED: &str =
    "Repairs cost 4.2 million pounds, of which the regional transport fund paid half.";
const CUTS_KEPT: &str =
    "Engineers replaced them one at a time so that the bridge never had to close, a zephyr said.";
const MISATTRIBUTED: &str = "Mayor Quillon said: \"The cables were replaced one at a time so that the bridge never had to close to pedestrians.\"";
const UNCONFIRMED: &str = "The bridge repairs were finished well ahead of a lantern schedule.";
const OWN: &str = "Half of 4.2 million pounds is 2.1 million pounds, a marigold sum.";
/// Words of the reply's sentences that are not in the release.
const REPLY_WORDS: [&str; 4] = ["zephyr", "Quillon", "lantern", "marigold"];

/// One item for each row the deterministic stages reach.
fn reply() -> Value {
    json!([
        {"kind": "quote", "passage": "p2"},
        {"kind": "quote", "passage": "one at a time so that the bridge"},
        {"kind": "quote", "passage": "p9"},
        {"kind": "from_source", "text": CONFIRMED, "passages": ["p2"]},
        {"kind": "from_source", "text": CUTS_KEPT, "passages": ["p1"]},
        {"kind": "from_source", "text": MISATTRIBUTED, "passages": ["p1"]},
        {"kind": "from_source", "text": UNCONFIRMED, "passages": ["p0"]},
        {"kind": "own", "text": OWN},
    ])
}

fn outcomes(display: &Value) -> Vec<String> {
    display["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["outcome"].as_str().unwrap().to_owned())
        .collect()
}

/// The counts vector in `docs/contracts/display-counts.md` order.
fn vector(display: &Value) -> Vec<u64> {
    display["counts"]["vector"]
        .as_array()
        .unwrap()
        .iter()
        .map(|count| count.as_u64().unwrap())
        .collect()
}

/// Every reply sentence's text in the display, with the label it is shown
/// under, and every other block's text from the release.
fn assert_built_from_the_release_and_the_rule(display: &Value, release_text: &str) {
    for block in display["blocks"].as_array().unwrap() {
        let text = block["text"].as_str().unwrap();
        match block["outcome"].as_str().unwrap() {
            "shown_confirmed" => {
                assert_eq!(block["label"], "confirmed from the article");
                assert!(release_text.contains(text), "{block}");
            }
            "shown_own" | "shown_unconfirmed" => {
                assert_eq!(block["label"], "not from the article");
            }
            "quote_refused" => {
                assert_eq!(text, "");
                assert!(block.get("passage").is_none(), "{block}");
            }
            _ => {
                assert!(release_text.contains(text), "{block}");
                let span = &block["span"];
                let (start, end) = (
                    span["start"].as_u64().unwrap() as usize,
                    span["end"].as_u64().unwrap() as usize,
                );
                assert_eq!(&release_text[start..end], text);
                assert!(
                    block["label"]
                        .as_str()
                        .unwrap()
                        .starts_with("from the source")
                );
            }
        }
    }
}

/// The recorded release, `strict`: fetched from loopback, then a reply
/// covering every row the deterministic stages reach displayed against it
/// by its acquisition handle. A sentence that is not confirmed, and the
/// assistant's own, are replaced by the source's passage; the display and
/// the record carry the outcomes and counts; the record carries no
/// sentence.
#[test]
fn a_reply_is_displayed_strictly_against_the_recorded_release() {
    let site = origin(recorded_body());
    let home = tempfile::tempdir().expect("tempdir");
    let mut edge = Edge::start(home.path(), "strict", &["--host-observations"]);
    let fetched = edge.fetch(&format!("{}/article", site.url()));
    let handle = fetched["acquisition_id"].as_str().expect("a handle");
    let text = recorded_text();
    assert_eq!(fetched["content_hash"], sha256_digest(text.as_bytes()));

    let answer = edge.show(handle, reply());
    assert_eq!(answer["result"]["isError"], false, "{answer}");
    let display = payload(&answer);
    assert_eq!(display, answer["result"]["structuredContent"]);
    assert_eq!(display["display_mode"], "strict");
    assert_eq!(display["release"]["content_hash"], fetched["content_hash"]);
    assert_eq!(display["release"]["acquisition_ids"], json!([handle]));
    assert_eq!(
        outcomes(&display),
        [
            "quote_shown",
            "quote_expanded_to_kept",
            "quote_refused",
            "shown_confirmed",
            "withheld_kept",
            "withheld_attribution",
            "withheld_unconfirmed",
            "withheld_unconfirmed",
        ]
    );
    assert_eq!(vector(&display), [1, 1, 1, 1, 1, 0, 0, 0, 1, 1, 2]);
    assert_eq!(display["counts"]["names"][10], "withheld_unconfirmed");
    assert_eq!(display["counts"]["status"], "working_rule");
    assert_built_from_the_release_and_the_rule(&display, &text);
    let confirmed = &display["blocks"][3];
    assert_eq!(confirmed["sources"][0]["passage"], "p2");
    assert_eq!(confirmed["text"], CONFIRMED);

    // Strict display shows no word of the reply that is not the release's,
    // in the payload or in the plain text.
    let plain = answer["result"]["content"][1]["text"].as_str().unwrap();
    assert!(
        plain.contains("[confirmed from the article; p2] Repairs cost"),
        "{plain}"
    );
    assert!(plain.contains("[not shown]"), "{plain}");
    assert!(plain.contains("withheld_unconfirmed 2"), "{plain}");
    for word in REPLY_WORDS {
        assert!(!plain.contains(word), "{word}: {plain}");
        assert!(!display.to_string().contains(word), "{word}: {display}");
    }
    assert!(!plain.contains('<'), "no markup: {plain}");
    // Nothing the screens did not rule on: no title, attribution, URL or
    // terms of the release.
    for metadata in [
        "Harbour Bridge Repairs",
        "source.example",
        "CC BY",
        "Attribute",
    ] {
        assert!(!display.to_string().contains(metadata), "{metadata}");
        assert!(!plain.contains(metadata), "{metadata}");
    }
    drop(edge);

    let log = records(home.path());
    let checker: Vec<(usize, &Value)> = log
        .iter()
        .enumerate()
        .filter(|(_, r)| r["payload"]["processor"]["name"] == "integrity-checker")
        .collect();
    assert_eq!(checker.len(), 1);
    let (at, checker) = checker[0];
    let rendered: Vec<&Value> = log
        .iter()
        .filter(|r| r["event"] == "display_rendered")
        .collect();
    assert_eq!(rendered.len(), 1);
    let rendered = &rendered[0]["payload"];
    assert_eq!(rendered["processor_record"], log[at]["seq"]);
    assert_eq!(rendered["outcomes"], json!(outcomes(&display)));
    assert_eq!(rendered["counts"], display["counts"]);
    assert_eq!(rendered["release"]["content_hash"], fetched["content_hash"]);
    assert_eq!(rendered["tool"], "context_show");
    assert_eq!(rendered["host"], "pi");
    assert_eq!(
        checker["payload"]["detail"]["counts_vector"],
        json!(vector(&display)),
        "the checker's record and the display agree"
    );

    let ndjson = std::fs::read_to_string(home.path().join("sessions/test-session.ndjson")).unwrap();
    for sentence in [CONFIRMED, CUTS_KEPT, MISATTRIBUTED, UNCONFIRMED, OWN] {
        assert!(
            !ndjson.contains(sentence),
            "a reply sentence entered the record"
        );
    }
    for word in REPLY_WORDS {
        assert!(!ndjson.contains(word), "{word} entered the record");
    }
    for paragraph in text.split("\n\n") {
        assert!(
            !ndjson.contains(paragraph),
            "the release text entered the record"
        );
    }
}

/// The same release signed for `labelled` display: the same reply shows the
/// unconfirmed sentence and the assistant's own under the label "not from
/// the article", by its content hash.
#[test]
fn a_reply_is_displayed_labelled_against_a_release_that_asks_for_it() {
    let body = resigned_body(|release| {
        release["sidecar"]["signed"]["display_mode"] = json!("labelled");
    });
    let site = origin(body);
    let home = tempfile::tempdir().expect("tempdir");
    let mut edge = Edge::start(home.path(), "strict", &[]);
    let fetched = edge.fetch(&format!("{}/article", site.url()));
    assert!(fetched.get("acquisition_id").is_none(), "{fetched}");
    let hash = fetched["content_hash"].as_str().unwrap().to_owned();

    let answer = edge.show(&hash, reply());
    assert_eq!(answer["result"]["isError"], false, "{answer}");
    let display = payload(&answer);
    assert_eq!(display["display_mode"], "labelled");
    assert_eq!(
        outcomes(&display),
        [
            "quote_shown",
            "quote_expanded_to_kept",
            "quote_refused",
            "shown_confirmed",
            "withheld_kept",
            "withheld_attribution",
            "shown_unconfirmed",
            "shown_own",
        ]
    );
    assert_eq!(vector(&display), [1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 0]);
    assert_built_from_the_release_and_the_rule(&display, &recorded_text());
    assert_eq!(display["blocks"][6]["text"], UNCONFIRMED);
    assert_eq!(display["blocks"][7]["text"], OWN);
    let plain = answer["result"]["content"][1]["text"].as_str().unwrap();
    assert!(
        plain.contains(&format!("[not from the article] {OWN}")),
        "{plain}"
    );
    assert!(
        plain.contains(&format!("[not from the article] {UNCONFIRMED}")),
        "{plain}"
    );
    // The withheld sentences' words still never reach the reader.
    for word in ["zephyr", "Quillon"] {
        assert!(!plain.contains(word), "{word}: {plain}");
    }
    drop(edge);
    let ndjson = std::fs::read_to_string(home.path().join("sessions/test-session.ndjson")).unwrap();
    for word in REPLY_WORDS {
        assert!(!ndjson.contains(word), "{word} entered the record");
    }
}

/// A display names only what this session delivered as a verified release:
/// an unknown handle, a plain page and a release whose signature failed are
/// each refused with the gap named, and none leaves a `display_rendered`
/// record.
#[test]
fn a_display_against_anything_but_a_delivered_verified_release_is_refused() {
    let tampered = {
        let mut release: Value = serde_json::from_slice(&recorded_body()).unwrap();
        let text = release["text"]
            .as_str()
            .unwrap()
            .replace("Monday", "Friday");
        release["text"] = json!(text);
        serde_json::to_vec_pretty(&release).unwrap()
    };
    let site = origin(tampered);
    let home = tempfile::tempdir().expect("tempdir");
    let mut edge = Edge::start(home.path(), "observe", &[]);
    let items = json!([{"kind": "own", "text": OWN}]);

    let unknown = error(&edge.show(&sha256_digest(recorded_text().as_bytes()), items.clone()));
    assert!(
        unknown.starts_with("refused: no context_fetch in this session delivered a signed release"),
        "{unknown}"
    );

    let page = edge.fetch(&format!("{}/page", site.url()));
    let plain = error(&edge.show(page["content_hash"].as_str().unwrap(), items.clone()));
    assert!(
        plain.contains("was not served as a signed release"),
        "{plain}"
    );

    // `observe` carries a release whose signature failed, with the breach;
    // the display refuses it.
    let carried = edge.fetch(&format!("{}/article", site.url()));
    assert_eq!(carried["release"]["verification"], "failed");
    let failed = error(&edge.show(carried["content_hash"].as_str().unwrap(), items));
    assert!(failed.contains("has a failed signature"), "{failed}");
    assert!(!failed.contains("marigold"), "{failed}");

    let tools = edge.ask(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"context_show"), "{names:?}");
    let status = payload(&edge.ask(call("context_status", json!({}))));
    assert!(
        status["tools"]
            .as_array()
            .unwrap()
            .contains(&json!("context_show")),
        "{status}"
    );
    drop(edge);

    let log = records(home.path());
    assert!(!log.iter().any(|r| r["event"] == "display_rendered"));
    let checker: Vec<&Value> = log
        .iter()
        .filter(|r| r["payload"]["processor"]["name"] == "integrity-checker")
        .collect();
    assert_eq!(
        checker.len(),
        1,
        "only the failed release reached the checker"
    );
    assert_eq!(checker[0]["payload"]["decision"], "abstain");
    assert_eq!(
        checker[0]["payload"]["gaps"][0]["reason"],
        "evidence_missing"
    );
}

/// A loopback origin serving whatever body `serving` holds at `/article`,
/// so that one URL can deliver different releases in turn.
fn changing_origin(serving: std::sync::Arc<std::sync::Mutex<Vec<u8>>>) -> ServerHandle {
    Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |request| {
            if request.target == "/robots.txt" {
                return Response::text(200, "User-agent: *\nAllow: /\n");
            }
            if request.target.starts_with("/.well-known/") {
                return Response::text(404, "not here");
            }
            let mut response = Response::new(200, serving.lock().unwrap().clone());
            response.headers.set("Content-Type", MEDIA_TYPE);
            response
        })
        .expect("spawn")
}

/// What a display by `key` showed, without the handles that name the
/// release (a later fetch of the same release adds one).
fn shown(answer: &Value) -> Value {
    let result = &answer["result"];
    if result["isError"] == true {
        return json!({"error": error(answer)});
    }
    let mut structured = result["structuredContent"].clone();
    structured["release"]
        .as_object_mut()
        .expect("the payload names the release")
        .remove("acquisition_ids");
    json!({"structured": structured, "plain": result["content"][1]["text"]})
}

/// Releases with the same text, each delivered in turn from one URL. Each
/// handle is displayed as soon as its fetch returns and again after every
/// later fetch, and must show the same thing each time; the content hash,
/// which several releases now share, is refused as ambiguous.
fn each_handle_keeps_its_release(bodies: &[Vec<u8>]) -> Vec<Value> {
    let serving = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let site = changing_origin(serving.clone());
    let home = tempfile::tempdir().expect("tempdir");
    let mut edge = Edge::start(home.path(), "observe", &["--host-observations"]);
    let url = format!("{}/article", site.url());
    let mut delivered: Vec<(String, Value)> = Vec::new();
    for body in bodies {
        *serving.lock().unwrap() = body.clone();
        let fetched = edge.fetch(&url);
        assert_eq!(
            fetched["content_hash"],
            sha256_digest(recorded_text().as_bytes()),
            "the same text each time"
        );
        let handle = fetched["acquisition_id"]
            .as_str()
            .expect("a handle")
            .to_owned();
        let first = edge.show(&handle, reply());
        for (earlier, showed) in &delivered {
            let again = edge.show(earlier, reply());
            assert_eq!(
                shown(&again),
                shown(showed),
                "a later release with the same text changed what {earlier} shows"
            );
        }
        delivered.push((handle, first));
    }
    let hash = sha256_digest(recorded_text().as_bytes());
    let ambiguous = error(&edge.show(&hash, reply()));
    assert!(
        ambiguous.starts_with(&format!(
            "refused: this server holds {} different signed releases with that content_hash",
            bodies.len()
        )),
        "{ambiguous}"
    );
    for (handle, _) in &delivered {
        assert!(ambiguous.contains(handle.as_str()), "{ambiguous}");
    }
    delivered.into_iter().map(|(_, answer)| answer).collect()
}

/// The same text under a different display mode, signed by the same key:
/// the strict handle stays strict and withholds the reply's own sentences.
#[test]
fn identical_text_under_another_display_mode_leaves_each_handle_its_own_mode() {
    let strict = resigned_body(|_| {});
    let labelled = resigned_body(|release| {
        release["sidecar"]["signed"]["display_mode"] = json!("labelled");
    });
    let shown = each_handle_keeps_its_release(&[strict, labelled]);
    let (strict, labelled) = (payload(&shown[0]), payload(&shown[1]));
    assert_eq!(strict["display_mode"], "strict");
    assert_eq!(labelled["display_mode"], "labelled");
    assert!(!strict.to_string().contains("marigold"), "{strict}");
    assert!(labelled.to_string().contains("marigold"), "{labelled}");
}

/// The same text with other signed passages or markers: each handle keeps
/// the passages and markers its fetch delivered.
#[test]
fn identical_text_with_other_passages_or_markers_leaves_each_handle_its_own() {
    let recorded = recorded_body();
    let fewer_passages = resigned_body(|release| {
        release["sidecar"]["signed"]["passages"]
            .as_array_mut()
            .unwrap()
            .retain(|passage| passage["id"] != "p2");
    });
    let no_markers = resigned_body(|release| {
        release["sidecar"]["signed"]["markers"] = json!([]);
    });
    let shown = each_handle_keeps_its_release(&[recorded, fewer_passages, no_markers]);
    let outcomes: Vec<Vec<String>> = shown.iter().map(|s| outcomes(&payload(s))).collect();
    assert_eq!(outcomes[0][0], "quote_shown");
    assert_eq!(outcomes[1][0], "quote_refused", "p2 is not signed there");
    assert_ne!(outcomes[2], outcomes[0], "the markers change the display");
}

/// The same text with a missing, a valid and a failed signature, carried
/// under `observe`: the missing and failed handles stay refused under their
/// own state, and the verified handle stays verified.
#[test]
fn identical_text_with_another_verification_state_leaves_each_handle_its_own() {
    let mut missing: Value = serde_json::from_slice(&recorded_body()).unwrap();
    missing["sidecar"]["signature"] = json!("");
    let mut failed: Value = serde_json::from_slice(&recorded_body()).unwrap();
    failed["sidecar"]["signature"] = json!(base64_zeroes());
    let shown = each_handle_keeps_its_release(&[
        serde_json::to_vec_pretty(&missing).unwrap(),
        recorded_body(),
        serde_json::to_vec_pretty(&failed).unwrap(),
    ]);
    assert!(error(&shown[0]).contains("has a missing signature"));
    assert_eq!(payload(&shown[1])["release"]["verification"], "verified");
    assert!(error(&shown[2]).contains("has a failed signature"));
}

/// The same release fetched twice is one release under both handles, and
/// its content hash still names it.
#[test]
fn the_same_release_fetched_again_is_held_once_under_both_handles() {
    let site = origin(recorded_body());
    let home = tempfile::tempdir().expect("tempdir");
    let mut edge = Edge::start(home.path(), "observe", &["--host-observations"]);
    let url = format!("{}/article", site.url());
    let first = edge.fetch(&url)["acquisition_id"].clone();
    let second = edge.fetch(&url)["acquisition_id"].clone();
    assert_ne!(first, second);
    let by_hash = edge.show(
        &sha256_digest(recorded_text().as_bytes()),
        json!([{"kind": "quote", "passage": "p2"}]),
    );
    assert_eq!(by_hash["result"]["isError"], false, "{by_hash}");
    assert_eq!(
        payload(&by_hash)["release"]["acquisition_ids"],
        json!([first, second])
    );
}

fn base64_zeroes() -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode([0u8; 64])
}
