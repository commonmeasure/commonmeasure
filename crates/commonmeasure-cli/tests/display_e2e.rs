//! `context_show` through the shipped stdio server: the recorded signed
//! release (`tests/recorded/signed-release/`) is fetched from a loopback
//! origin through the production transport and the release verifier, then a
//! reply is displayed against it by its acquisition handle or content hash.
//!
//! The recorded release asks for `strict` display. The `labelled` mode is
//! the same release with its display mode changed and signed again under a
//! test key, served and verified the same way.
//!
//! Each display's C2PA manifest (`docs/contracts/display-provenance.md`) is
//! signed with the committed demonstration identity
//! (`demo/provenance/signer.pem`) where a test names it, and a release's own
//! C2PA store with a publisher certificate generated per run.

mod common;
#[path = "support/sidecar_display.rs"]
mod sidecar_display;
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

/// The variables a display's manifest is signed and read back under.
const PROVENANCE_VARIABLES: [&str; 4] = [
    "COMMONMEASURE_PROVENANCE_CERTIFICATE",
    "COMMONMEASURE_PROVENANCE_KEY",
    "COMMONMEASURE_PROVENANCE_SIGNER",
    "COMMONMEASURE_PROVENANCE_TRUST_ANCHORS",
];

impl Edge {
    /// The server with no signing identity: every display's manifest is
    /// unavailable.
    fn start(home: &Path, mode: &str, extra: &[&str]) -> Self {
        Self::start_with(home, mode, extra, &[])
    }

    /// The server with `environment` set over none of the provenance
    /// variables.
    fn start_with(home: &Path, mode: &str, extra: &[&str], environment: &[(&str, &Path)]) -> Self {
        write_policy(
            home,
            &format!(r#"{{"policy_mode":"{mode}","allow_private_hosts":true}}"#),
        );
        Self::start_under_policy(home, extra, environment)
    }

    fn start_under_policy(home: &Path, extra: &[&str], environment: &[(&str, &Path)]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_commonmeasure"));
        for variable in PROVENANCE_VARIABLES {
            command.env_remove(variable);
        }
        for variable in [
            "COMMONMEASURE_CHECKER",
            "COMMONMEASURE_CHECKER_SUMMARY",
            "COMMONMEASURE_CHECKER_TARGET",
            "COMMONMEASURE_CHECKER_THRESHOLD",
            "COMMONMEASURE_CHECKER_CONTRADICTION_THRESHOLD",
            "COMMONMEASURE_CHECKER_TIMEOUT_SECONDS",
        ] {
            command.env_remove(variable);
        }
        command.envs(environment.iter().copied());
        let mut child = command
            .args(["mcp", "--host", "pi", "--session", "test-session"])
            .args(extra)
            .env("COMMONMEASURE_HOME", home)
            .env_remove("AGENT_SESSION_ID")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(std::fs::File::create(home.join("stderr.log")).unwrap())
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

    // No signing identity: the display stands, its manifest is unavailable,
    // the output-provenance record abstains naming both variables, and no
    // sidecar is written.
    assert_eq!(display["provenance"]["manifest"], Value::Null, "{display}");
    assert_eq!(display["provenance"]["validation"]["state"], "unavailable");
    let provenance = provenance_record(&log, rendered);
    assert_eq!(display["provenance"]["record"], provenance["seq"]);
    let provenance = &provenance["payload"];
    assert_eq!(provenance["decision"], "abstain", "{provenance}");
    assert_eq!(provenance["outputs"], json!([]));
    let gap = provenance["gaps"][0]["detail"].as_str().unwrap();
    assert!(
        gap.contains("COMMONMEASURE_PROVENANCE_CERTIFICATE")
            && gap.contains("COMMONMEASURE_PROVENANCE_KEY"),
        "{gap}"
    );
    assert_eq!(
        rendered["display_text_sha256"],
        commonmeasure_runtime::processor::provenance::display_text_sha256(plain)
    );
    assert!(!home.path().join("sessions/test-session.displays").exists());

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
    // Each display has its own provenance record.
    structured
        .as_object_mut()
        .expect("the payload is an object")
        .remove("provenance");
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
            "refused: this server holds {} different deliveries of a signed release with that \
             content_hash",
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

/// The output-provenance record a `display_rendered` payload names.
fn provenance_record<'a>(log: &'a [Value], rendered: &Value) -> &'a Value {
    let named = &rendered["provenance_record"];
    assert!(named.is_u64(), "{rendered}");
    let record = log
        .iter()
        .find(|record| record["seq"] == *named)
        .expect("the named record is in the log");
    assert_eq!(record["event"], "processor_invoked");
    assert_eq!(record["payload"]["processor"]["name"], "output-provenance");
    record
}

fn repo_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/commonmeasure-cli sits two levels below the repository root")
        .to_path_buf()
}

/// The committed demonstration identity's chain and key, and a file holding
/// its root alone, the trust anchor under which its manifests read
/// `trusted`.
fn demonstration_identity(directory: &Path) -> [(&'static str, std::path::PathBuf); 3] {
    let chain = repo_root().join("demo/provenance/signer.pem");
    let pem = std::fs::read_to_string(&chain).unwrap();
    let root = pem
        .rfind("-----BEGIN CERTIFICATE-----")
        .map(|at| pem[at..].to_owned())
        .unwrap();
    let anchors = directory.join("anchors.pem");
    std::fs::write(&anchors, root).unwrap();
    [
        ("COMMONMEASURE_PROVENANCE_CERTIFICATE", chain),
        (
            "COMMONMEASURE_PROVENANCE_KEY",
            repo_root().join("demo/provenance/signer-key.pem"),
        ),
        ("COMMONMEASURE_PROVENANCE_TRUST_ANCHORS", anchors),
    ]
}

/// A stand-in publisher's certificate chain and key, generated per run with
/// the key usage the C2PA certificate profile requires, and its root's PEM.
fn publisher() -> (
    commonmeasure_runtime::processor::provenance::SigningMaterial,
    String,
) {
    use rcgen::{
        BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose,
        IsCa, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
    };
    let named = |common_name: &str| {
        let mut name = DistinguishedName::new();
        name.push(DnType::CommonName, common_name);
        name.push(DnType::OrganizationName, "Common Measure Ltd");
        name
    };
    let root_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut root = CertificateParams::default();
    root.distinguished_name = named("Stand-in publisher test root");
    root.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    root.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let root = root.self_signed(&root_key).unwrap();
    let leaf_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut leaf = CertificateParams::default();
    leaf.distinguished_name = named("Stand-in publisher, test");
    leaf.is_ca = IsCa::ExplicitNoCa;
    leaf.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::EmailProtection];
    leaf.use_authority_key_identifier_extension = true;
    let leaf = leaf.signed_by(&leaf_key, &root, &root_key).unwrap();
    let material = commonmeasure_runtime::processor::provenance::SigningMaterial {
        certificate_chain: format!("{}{}", leaf.pem(), root.pem()).into_bytes(),
        private_key: leaf_key.serialize_pem().into_bytes(),
        certificate_ref: "ephemeral stand-in publisher certificate".to_owned(),
        key_ref: "ephemeral stand-in publisher key".to_owned(),
    };
    (material, root.pem())
}

/// The recorded release with a structured usage block (the owner's example:
/// AI input allowed, an onward commercial product not) and, where `text`
/// is given, a C2PA store the publisher signed over that text.
fn release_with_store(
    publisher: &commonmeasure_runtime::processor::provenance::SigningMaterial,
    text: Option<&str>,
) -> Vec<u8> {
    use base64::Engine as _;
    use commonmeasure_runtime::processor::provenance::{TrainingMining, Use, sign_source_store};
    let store = text.map(|text| {
        let declared = TrainingMining {
            ai_inference: Use::Allowed,
            ai_training: Use::NotAllowed,
            ai_generative_training: Use::NotAllowed,
            data_mining: Use::NotAllowed,
            constraint_info: None,
        };
        let store = sign_source_store("Harbour release", text, Some(&declared), publisher)
            .expect("the publisher signs");
        base64::engine::general_purpose::STANDARD.encode(store)
    });
    resigned_body(|release| {
        release["sidecar"]["signed"]["usage"] = json!({
            "ai_input": "allow",
            "onward_commercial_product": "disallow",
            "reporting": {"required": true, "destination": "https://source.example/report"},
        });
        if let Some(store) = store {
            release["sidecar"]["c2pa"] = json!(store);
        }
    })
}

/// Run the binary on `arguments`, returning its exit status and stdout.
fn commonmeasure(arguments: &[&std::ffi::OsStr]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(arguments)
        .output()
        .expect("the binary should start");
    (
        output.status.success(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// A display with the demonstration identity configured: one manifest,
/// signed, read back `trusted` under its root, written beside the session
/// log and named by the `display_rendered` record and the payload. The
/// manifest carries no identity, the release as its ingredient by content
/// hash, grade and sidecar digest, and the terms with the derived statement.
/// `inspect` and `provenance` read the session back with line citations.
/// Neither the log nor the sidecar holds a sentence of the release or the
/// reply.
#[test]
fn a_display_s_manifest_is_signed_written_beside_the_log_and_read_back() {
    use commonmeasure_runtime::processor::provenance::{
        TERMS_LABEL, read_store_unbound, read_store_with_trust,
    };
    let (publisher, _) = publisher();
    let site = origin(release_with_store(&publisher, None));
    let home = tempfile::tempdir().expect("tempdir");
    let identity = demonstration_identity(home.path());
    let environment: Vec<(&str, &Path)> = identity
        .iter()
        .map(|(name, path)| (*name, path.as_path()))
        .collect();
    let mut edge = Edge::start_with(home.path(), "strict", &[], &environment);
    let fetched = edge.fetch(&format!("{}/article", site.url()));
    let hash = fetched["content_hash"].as_str().unwrap().to_owned();
    let answer = edge.show(&hash, reply());
    assert_eq!(answer["result"]["isError"], false, "{answer}");
    let display = payload(&answer);
    let plain = answer["result"]["content"][1]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    drop(edge);

    let log = records(home.path());
    let rendered: Vec<(usize, &Value)> = log
        .iter()
        .enumerate()
        .filter(|(_, r)| r["event"] == "display_rendered")
        .collect();
    assert_eq!(rendered.len(), 1);
    let (rendered_line, rendered) = rendered[0];
    let rendered = &rendered["payload"];
    let record = provenance_record(&log, rendered);
    let provenance_line = log.iter().position(|r| r["seq"] == record["seq"]).unwrap();
    assert!(
        provenance_line < rendered_line,
        "written before the display it names"
    );
    let invocation = &record["payload"];
    assert_eq!(invocation["decision"], "admit", "{invocation}");
    assert_eq!(invocation["detail"]["validation"]["state"], "trusted");
    assert_eq!(invocation["detail"]["signing"]["alg"], "ES256");
    let display_text_sha256 =
        commonmeasure_runtime::processor::provenance::display_text_sha256(&plain);
    assert_eq!(rendered["display_text_sha256"], display_text_sha256);
    assert_eq!(
        invocation["detail"]["binding"]["display_text_sha256"],
        display_text_sha256
    );

    // The payload names the record and the sidecar; the bytes stay out of
    // the tool result.
    let reference = invocation["outputs"][0]["reference"].as_str().unwrap();
    assert_eq!(display["provenance"]["record"], record["seq"]);
    assert_eq!(display["provenance"]["manifest"]["reference"], reference);
    assert_eq!(display["provenance"]["validation"]["state"], "trusted");
    let sidecar = home.path().join("sessions").join(reference);
    assert!(
        reference.starts_with("test-session.displays/") && reference.ends_with(".c2pa"),
        "{reference}"
    );
    let store = std::fs::read(&sidecar).expect("the sidecar is written");
    assert_eq!(
        display["provenance"]["manifest"]["sha256"],
        sha256_digest(&store)
    );
    assert_eq!(display["provenance"]["manifest"]["bytes"], store.len());
    assert_eq!(
        invocation["outputs"][0]["content_hash"],
        sha256_digest(&store)
    );

    // The SDK reader validates the sidecar against the display text.
    let anchors = std::fs::read_to_string(&identity[2].1).unwrap();
    let reading = read_store_with_trust(&store, &plain, Some(&anchors)).unwrap();
    assert_eq!(
        reading.validation["state"], "trusted",
        "{}",
        reading.validation
    );
    assert!(
        read_store_unbound(&store, None)
            .unwrap()
            .assertions("cawg.identity")
            .is_empty(),
        "no identity for the reader or anyone else"
    );
    let ingredient = &reading.active()["ingredients"][0];
    assert_eq!(ingredient["instance_id"], fetched["content_hash"]);
    assert_eq!(ingredient["description"], "mediated");
    assert_eq!(ingredient["relationship"], "inputTo");
    assert_eq!(ingredient["format"], MEDIA_TYPE);
    let checker = log
        .iter()
        .find(|r| r["payload"]["processor"]["name"] == "integrity-checker")
        .unwrap();
    let sidecar_input = checker["payload"]["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|input| input["reference"].as_str().unwrap().ends_with("#sidecar"))
        .expect("the checker records the sidecar digest");
    assert_eq!(
        ingredient["metadata"]["ai.commonmeasure.sidecar_digest"],
        sidecar_input["content_hash"]
    );
    let terms = reading.assertion(TERMS_LABEL);
    assert_eq!(
        terms["ingredients"][0]["instance_id"],
        fetched["content_hash"]
    );
    assert_eq!(terms["ingredients"][0]["usage"]["ai_input"], "allow");
    assert_eq!(terms["derived"]["onward_commercial_product"], "disallow");
    assert_eq!(terms["derived"]["reporting"]["required"], true);
    assert_eq!(
        terms["derived"]["reporting"]["destinations"],
        json!(["https://source.example/report"])
    );
    let mining = reading.assertion("cawg.training-mining");
    assert_eq!(mining["entries"]["cawg.ai_inference"]["use"], "allowed");

    // Nothing of the release or the reply in the log or the sidecar.
    let ndjson = std::fs::read_to_string(home.path().join("sessions/test-session.ndjson")).unwrap();
    let bytes = String::from_utf8_lossy(&store);
    for paragraph in recorded_text().split("\n\n") {
        assert!(
            !ndjson.contains(paragraph),
            "the release text entered the record"
        );
    }
    for sentence in recorded_text()
        .split(['.', '\n'])
        .map(str::trim)
        .filter(|sentence| sentence.len() > 12)
    {
        assert!(!bytes.contains(sentence), "{sentence} entered the sidecar");
    }
    for line in plain.lines().filter(|line| line.len() > 24) {
        let words = line.rsplit("] ").next().unwrap();
        if words.len() > 12 {
            assert!(!bytes.contains(words), "{words} entered the sidecar");
            assert!(!ndjson.contains(words), "{words} entered the record");
        }
    }
    for word in REPLY_WORDS {
        assert!(!bytes.contains(word), "{word} entered the sidecar");
    }

    // `inspect` cites both records by line.
    let log_path = home.path().join("sessions/test-session.ndjson");
    let (ok, dossier, stderr) = commonmeasure(&["inspect".as_ref(), log_path.as_os_str()]);
    assert!(ok, "{stderr}");
    let at = |line: usize| format!("{}:{}", log_path.display(), line + 1);
    assert!(
        dossier.contains(&format!(
            "strict display of release {hash} [{}]",
            at(rendered_line)
        )),
        "{dossier}"
    );
    assert!(
        dossier.contains(&format!(
            "manifest: {reference} {} ({} bytes), validation trusted at write time",
            sha256_digest(&store),
            store.len()
        )),
        "{dossier}"
    );
    assert!(
        dossier.contains(&format!("[{}]", at(provenance_line))),
        "{dossier}"
    );
    assert!(dossier.contains("withheld_unconfirmed 2"), "{dossier}");

    // `provenance` reads the sidecar back without the text: the claim
    // verifies and the binding is reported as not re-verified.
    let anchors_path = identity[2].1.as_os_str();
    let (ok, report, stderr) = commonmeasure(&[
        "provenance".as_ref(),
        log_path.as_os_str(),
        "--trust-anchors".as_ref(),
        anchors_path,
    ]);
    assert!(ok, "{stderr}");
    let report: Value = serde_json::from_str(&report).unwrap();
    let entry = &report["displays"][0];
    assert_eq!(entry["display"]["record"], at(rendered_line));
    let manifest = &entry["manifest"];
    assert_eq!(manifest["record"], at(provenance_line));
    assert_eq!(manifest["matches_record"], true);
    assert_eq!(manifest["validation"]["state"], "trusted", "{manifest}");
    assert_eq!(manifest["binding"]["re_verified"], false);
    assert_eq!(manifest["binding"]["at_write_time"], "trusted");
    assert_eq!(manifest["identity"], Value::Null);
    assert_eq!(
        manifest["ingredients"][0]["instance_id"],
        fetched["content_hash"]
    );
    assert_eq!(manifest["terms"]["derived"], terms["derived"]);
    assert_eq!(
        manifest["signer"]["common_name"],
        "Common Measure demonstration signer"
    );

    // With the text the host kept, the binding is re-verified; another
    // text fails.
    let kept = home.path().join("kept.txt");
    std::fs::write(&kept, &plain).unwrap();
    let (ok, report, stderr) = commonmeasure(&[
        "provenance".as_ref(),
        log_path.as_os_str(),
        "--text".as_ref(),
        kept.as_os_str(),
    ]);
    assert!(ok, "{stderr}");
    let report: Value = serde_json::from_str(&report).unwrap();
    let manifest = &report["displays"][0]["manifest"];
    assert_eq!(manifest["binding"]["re_verified"], true, "{manifest}");
    assert_eq!(manifest["validation"]["state"], "valid", "{manifest}");
    std::fs::write(&kept, "Another display.").unwrap();
    let (ok, _, stderr) = commonmeasure(&[
        "provenance".as_ref(),
        log_path.as_os_str(),
        "--text".as_ref(),
        kept.as_os_str(),
    ]);
    assert!(!ok);
    assert!(stderr.contains("no display"), "{stderr}");

    // A sidecar altered after its record fails the report.
    let mut damaged = store.clone();
    damaged[10] ^= 0xff;
    std::fs::write(&sidecar, damaged).unwrap();
    let (ok, _, stderr) = commonmeasure(&["provenance".as_ref(), log_path.as_os_str()]);
    assert!(!ok);
    assert!(stderr.contains("does not hash to the record"), "{stderr}");
}

/// A release carrying its own C2PA store, signed by its publisher with a
/// `cawg.identity`: the release verifier records the store `trusted` under
/// the publisher's root with the identity on the crossing, and the display's
/// ingredient carries the store. A store bound to another text leaves the
/// release verified with the store `invalid`, and the ingredient carries
/// none.
#[test]
fn a_release_s_own_manifest_travels_with_its_ingredient_and_an_invalid_one_does_not() {
    use commonmeasure_runtime::processor::provenance::read_store_unbound;
    let (publisher, publisher_root) = publisher();
    for (case, signed_over) in [
        ("valid", recorded_text()),
        ("invalid", "Another text altogether.".to_owned()),
    ] {
        let site = origin(release_with_store(&publisher, Some(&signed_over)));
        let home = tempfile::tempdir().expect("tempdir");
        let identity = demonstration_identity(home.path());
        // The edge trusts the publisher's root as well as the claim signer's.
        let anchors = format!(
            "{}{publisher_root}",
            std::fs::read_to_string(&identity[2].1).unwrap()
        );
        std::fs::write(&identity[2].1, anchors).unwrap();
        let environment: Vec<(&str, &Path)> = identity
            .iter()
            .map(|(name, path)| (*name, path.as_path()))
            .collect();
        let mut edge = Edge::start_with(home.path(), "strict", &[], &environment);
        let fetched = edge.fetch(&format!("{}/article", site.url()));
        assert_eq!(fetched["release"]["verification"], "verified", "{case}");
        let answer = edge.show(
            fetched["content_hash"].as_str().unwrap(),
            json!([{"kind": "quote", "passage": "p2"}]),
        );
        assert_eq!(answer["result"]["isError"], false, "{case}: {answer}");
        let display = payload(&answer);
        drop(edge);

        let log = records(home.path());
        let crossing = log
            .iter()
            .find(|r| r["event"] == "crossing_mediated")
            .expect("the crossing");
        let c2pa = &crossing["payload"]["release"]["c2pa"];
        let sidecar = home.path().join("sessions").join(
            display["provenance"]["manifest"]["reference"]
                .as_str()
                .unwrap(),
        );
        let reading = read_store_unbound(&std::fs::read(sidecar).unwrap(), None).unwrap();
        let ingredient = &reading.active()["ingredients"][0];
        if case == "valid" {
            assert_eq!(c2pa["state"], "trusted", "{c2pa}");
            assert_eq!(c2pa["binding"]["matches"], true, "{c2pa}");
            assert_eq!(
                c2pa["identities"][0]["subject"], "Stand-in publisher, test",
                "{c2pa}"
            );
            assert!(!ingredient["active_manifest"].is_null(), "{ingredient}");
            let source =
                &reading.manifest["manifests"][ingredient["active_manifest"].as_str().unwrap()];
            assert!(
                source["assertions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|assertion| assertion["label"] == "cawg.identity"),
                "the source's identity travels with its manifest: {source}"
            );
            // Only the source's manifest names anyone; the display's does not.
            assert!(reading.assertions("cawg.identity").is_empty());
        } else {
            assert_eq!(c2pa["state"], "invalid", "{c2pa}");
            assert!(ingredient["active_manifest"].is_null(), "{ingredient}");
        }
        let ndjson =
            std::fs::read_to_string(home.path().join("sessions/test-session.ndjson")).unwrap();
        for paragraph in recorded_text().split("\n\n") {
            assert!(
                !ndjson.contains(paragraph),
                "the release text entered the record"
            );
        }
    }
}

/// A loopback origin serving the recorded release at `/article` under the
/// `Content-Usage` header `usage` holds at the time of each request.
fn declaring_origin(usage: std::sync::Arc<std::sync::Mutex<&'static str>>) -> ServerHandle {
    Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |request| {
            if request.target == "/robots.txt" {
                return Response::text(200, "User-agent: *\nAllow: /\n");
            }
            if request.target.starts_with("/.well-known/") {
                return Response::text(404, "not here");
            }
            let mut response = Response::new(200, recorded_body());
            response.headers.set("Content-Type", MEDIA_TYPE);
            response
                .headers
                .set("Content-Usage", *usage.lock().unwrap());
            response
        })
        .expect("spawn")
}

/// The signed store a display's payload names, read back without its text.
fn displayed_store(
    home: &Path,
    display: &Value,
) -> commonmeasure_runtime::processor::provenance::StoreReading {
    let reference = display["provenance"]["manifest"]["reference"]
        .as_str()
        .unwrap_or_else(|| panic!("a manifest was written: {display}"));
    let store = std::fs::read(home.join("sessions").join(reference)).expect("the sidecar");
    assert_eq!(
        display["provenance"]["manifest"]["sha256"],
        sha256_digest(&store)
    );
    commonmeasure_runtime::processor::provenance::read_store_unbound(&store, None).unwrap()
}

/// The same release bytes at the same URL fetched twice, first under a
/// header that allows training and then under one that denies it: each
/// handle's display signs the terms its own crossing recorded, and the
/// bare content hash, which matches both deliveries, is refused as
/// ambiguous with nothing signed for it.
#[test]
fn a_release_fetched_again_under_other_declarations_is_signed_under_its_own() {
    use commonmeasure_runtime::processor::provenance::{TERMS_LABEL, TRAINING_MINING_LABEL};
    let usage = std::sync::Arc::new(std::sync::Mutex::new("ai-use=y, train-ai=y"));
    let site = declaring_origin(usage.clone());
    let home = tempfile::tempdir().expect("tempdir");
    let identity = demonstration_identity(home.path());
    let environment: Vec<(&str, &Path)> = identity
        .iter()
        .map(|(name, path)| (*name, path.as_path()))
        .collect();
    let mut edge = Edge::start_with(
        home.path(),
        "strict",
        &["--host-observations"],
        &environment,
    );
    let url = format!("{}/article", site.url());
    let allowed = edge.fetch(&url);
    *usage.lock().unwrap() = "ai-use=y, train-ai=n";
    let denied = edge.fetch(&url);
    assert_eq!(allowed["content_hash"], denied["content_hash"]);
    let items = json!([{"kind": "quote", "passage": "p2"}]);
    let second = edge.show(denied["acquisition_id"].as_str().unwrap(), items.clone());
    let first = edge.show(allowed["acquisition_id"].as_str().unwrap(), items.clone());
    let ambiguous = error(&edge.show(denied["content_hash"].as_str().unwrap(), items));
    drop(edge);

    for (answer, fetched, training, header) in [
        (&second, &denied, "notAllowed", "ai-use=y, train-ai=n"),
        (&first, &allowed, "allowed", "ai-use=y, train-ai=y"),
    ] {
        let display = payload(answer);
        assert_eq!(
            display["release"]["acquisition_ids"],
            json!([fetched["acquisition_id"]]),
            "a handle keeps its own delivery"
        );
        let reading = displayed_store(home.path(), &display);
        let mining = reading.assertion(TRAINING_MINING_LABEL);
        for entry in ["cawg.ai_training", "cawg.ai_generative_training"] {
            assert_eq!(mining["entries"][entry]["use"], training, "{mining}");
        }
        let declarations = &reading.assertion(TERMS_LABEL)["ingredients"][0]["declarations"];
        assert_eq!(declarations["content_usage"], header, "{declarations}");
        assert_eq!(
            declarations["effective"]["train-ai"],
            if training == "allowed" {
                "allow"
            } else {
                "disallow"
            }
        );
    }

    assert!(
        ambiguous.starts_with(
            "refused: this server holds 2 different deliveries of a signed release with that \
             content_hash"
        ),
        "{ambiguous}"
    );
    for fetched in [&allowed, &denied] {
        assert!(
            ambiguous.contains(fetched["acquisition_id"].as_str().unwrap()),
            "{ambiguous}"
        );
    }
    let log = records(home.path());
    assert_eq!(
        log.iter()
            .filter(|r| r["event"] == "display_rendered")
            .count(),
        2,
        "the ambiguous call displayed nothing"
    );
    assert_eq!(
        log.iter()
            .filter(|r| r["payload"]["processor"]["name"] == "output-provenance")
            .count(),
        2,
        "and signed nothing"
    );
}

/// Two servers on one session, each fetching and displaying a different
/// passage: every record has its own `seq`, each display names its own
/// output-provenance record and its own store, neither store replaces the
/// other, and `provenance` reads both back and exits 0, trusted under the
/// demonstration root.
#[test]
fn two_servers_on_one_session_keep_their_own_manifests() {
    let site = origin(recorded_body());
    let home = tempfile::tempdir().expect("tempdir");
    let identity = demonstration_identity(home.path());
    let environment: Vec<(&str, &Path)> = identity
        .iter()
        .map(|(name, path)| (*name, path.as_path()))
        .collect();
    let url = format!("{}/article", site.url());
    let mut a = Edge::start_with(
        home.path(),
        "strict",
        &["--host-observations"],
        &environment,
    );
    let mut b = Edge::start_with(
        home.path(),
        "strict",
        &["--host-observations"],
        &environment,
    );
    // Each server holds the one delivery it fetched, so its content hash
    // names it; the second server on a session is not given a handle.
    let a_fetched = a.fetch(&url);
    let b_fetched = b.fetch(&url);
    let a_display = payload(&a.show(
        a_fetched["content_hash"].as_str().unwrap(),
        json!([{"kind": "quote", "passage": "p2"}]),
    ));
    let b_display = payload(&b.show(
        b_fetched["content_hash"].as_str().unwrap(),
        json!([{"kind": "quote", "passage": "p0"}]),
    ));
    drop(a);
    drop(b);

    let log = records(home.path());
    for (line, record) in log.iter().enumerate() {
        assert_eq!(record["seq"], json!(line + 1), "{record}");
    }
    assert_ne!(
        a_display["provenance"]["record"],
        b_display["provenance"]["record"]
    );
    assert_ne!(
        a_display["provenance"]["manifest"]["reference"],
        b_display["provenance"]["manifest"]["reference"]
    );
    assert_ne!(
        a_display["provenance"]["manifest"]["sha256"],
        b_display["provenance"]["manifest"]["sha256"]
    );
    for display in [&a_display, &b_display] {
        displayed_store(home.path(), display);
        let rendered = log
            .iter()
            .find(|r| {
                r["event"] == "display_rendered"
                    && r["payload"]["provenance_record"] == display["provenance"]["record"]
            })
            .map(|r| &r["payload"])
            .unwrap_or_else(|| panic!("a display_rendered record names {display}"));
        let record = provenance_record(&log, rendered);
        assert_eq!(
            record["payload"]["outputs"][0]["content_hash"],
            display["provenance"]["manifest"]["sha256"]
        );
    }
    let partials: Vec<_> = std::fs::read_dir(home.path().join("sessions/test-session.displays"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".partial"))
        .collect();
    assert!(partials.is_empty(), "{partials:?}");

    let log_path = home.path().join("sessions/test-session.ndjson");
    let (ok, report, stderr) = commonmeasure(&[
        "provenance".as_ref(),
        log_path.as_os_str(),
        "--trust-anchors".as_ref(),
        identity[2].1.as_os_str(),
        "--require-trusted".as_ref(),
    ]);
    assert!(ok, "{stderr}");
    let report: Value = serde_json::from_str(&report).unwrap();
    let displays = report["displays"].as_array().unwrap();
    assert_eq!(displays.len(), 2);
    for (entry, display) in displays.iter().zip([&a_display, &b_display]) {
        assert_eq!(entry["manifest"]["matches_record"], true, "{entry}");
        assert_eq!(
            entry["manifest"]["reference"],
            display["provenance"]["manifest"]["reference"]
        );
        assert_eq!(entry["manifest"]["validation"]["state"], "trusted");
    }
}

/// A display's store is never installed over one already at its name: the
/// display is still rendered, its record abstains with the gap, and the
/// store already there is left as it was.
#[test]
fn a_display_s_store_never_replaces_one_already_at_its_name() {
    let site = origin(recorded_body());
    let home = tempfile::tempdir().expect("tempdir");
    let identity = demonstration_identity(home.path());
    let environment: Vec<(&str, &Path)> = identity
        .iter()
        .map(|(name, path)| (*name, path.as_path()))
        .collect();
    let mut edge = Edge::start_with(
        home.path(),
        "strict",
        &["--host-observations"],
        &environment,
    );
    let fetched = edge.fetch(&format!("{}/article", site.url()));
    // The integrity checker's record is the next line, and names the store.
    let next = records(home.path()).len() + 1;
    let displays = home.path().join("sessions/test-session.displays");
    std::fs::create_dir_all(&displays).unwrap();
    let occupied = displays.join(format!("{next}.c2pa"));
    std::fs::write(&occupied, b"an earlier store").unwrap();
    let answer = edge.show(
        fetched["acquisition_id"].as_str().unwrap(),
        json!([{"kind": "quote", "passage": "p2"}]),
    );
    drop(edge);
    assert_eq!(answer["result"]["isError"], false, "{answer}");
    let display = payload(&answer);
    assert_eq!(display["provenance"]["manifest"], Value::Null, "{display}");
    assert_eq!(display["provenance"]["validation"]["state"], "unavailable");
    assert_eq!(std::fs::read(&occupied).unwrap(), b"an earlier store");
    let log = records(home.path());
    let rendered = log
        .iter()
        .find(|r| r["event"] == "display_rendered")
        .unwrap();
    assert_eq!(rendered["payload"]["processor_record"], next);
    let invocation = &provenance_record(&log, &rendered["payload"])["payload"];
    assert_eq!(invocation["decision"], "abstain");
    assert!(
        invocation["gaps"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("already there and is not replaced"),
        "{invocation}"
    );
}

/// A trust-anchors file that is configured but cannot be read is recorded
/// as a gap that names it, never as no anchors configured; the display is
/// rendered and its manifest is not trusted.
#[test]
fn configured_trust_anchors_that_cannot_be_read_are_a_named_gap() {
    let site = origin(recorded_body());
    let home = tempfile::tempdir().expect("tempdir");
    let identity = demonstration_identity(home.path());
    let missing = home.path().join("no-such-anchors.pem");
    let environment: Vec<(&str, &Path)> = vec![
        (identity[0].0, identity[0].1.as_path()),
        (identity[1].0, identity[1].1.as_path()),
        ("COMMONMEASURE_PROVENANCE_TRUST_ANCHORS", missing.as_path()),
    ];
    let mut edge = Edge::start_with(
        home.path(),
        "strict",
        &["--host-observations"],
        &environment,
    );
    let fetched = edge.fetch(&format!("{}/article", site.url()));
    let answer = edge.show(
        fetched["acquisition_id"].as_str().unwrap(),
        json!([{"kind": "quote", "passage": "p2"}]),
    );
    drop(edge);
    assert_eq!(answer["result"]["isError"], false, "{answer}");
    let display = payload(&answer);
    assert_ne!(display["provenance"]["validation"]["state"], "trusted");
    let log = records(home.path());
    let rendered = log
        .iter()
        .find(|r| r["event"] == "display_rendered")
        .unwrap();
    let invocation = &provenance_record(&log, &rendered["payload"])["payload"];
    let validation = &invocation["detail"]["validation"];
    assert_eq!(validation["trusted"], false, "{validation}");
    assert_eq!(
        validation["trust_list"], "configured trust anchors unavailable",
        "{validation}"
    );
    let gap = invocation["gaps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|gap| {
            gap["detail"]
                .as_str()
                .is_some_and(|detail| detail.starts_with("trust anchors unavailable"))
        })
        .unwrap_or_else(|| panic!("a gap names the anchors: {invocation}"));
    assert_eq!(gap["reason"], "capability_unavailable");
    assert!(
        gap["detail"]
            .as_str()
            .unwrap()
            .contains("no-such-anchors.pem"),
        "{gap}"
    );
}

/// `provenance --require-trusted` fails a display that has no manifest; the
/// same session without the flag reports it and exits 0.
#[test]
fn require_trusted_fails_a_display_without_a_manifest() {
    let site = origin(recorded_body());
    let home = tempfile::tempdir().expect("tempdir");
    let mut edge = Edge::start(home.path(), "strict", &["--host-observations"]);
    let fetched = edge.fetch(&format!("{}/article", site.url()));
    let answer = edge.show(
        fetched["acquisition_id"].as_str().unwrap(),
        json!([{"kind": "quote", "passage": "p2"}]),
    );
    drop(edge);
    assert_eq!(answer["result"]["isError"], false, "{answer}");
    let log_path = home.path().join("sessions/test-session.ndjson");
    let (ok, report, stderr) = commonmeasure(&["provenance".as_ref(), log_path.as_os_str()]);
    assert!(ok, "{stderr}");
    let report: Value = serde_json::from_str(&report).unwrap();
    assert_eq!(report["displays"][0]["manifest"]["state"], "unavailable");
    let identity = demonstration_identity(home.path());
    let (ok, _, stderr) = commonmeasure(&[
        "provenance".as_ref(),
        log_path.as_os_str(),
        "--trust-anchors".as_ref(),
        identity[2].1.as_os_str(),
        "--require-trusted".as_ref(),
    ]);
    assert!(!ok);
    assert!(stderr.contains("has no manifest"), "{stderr}");
}
