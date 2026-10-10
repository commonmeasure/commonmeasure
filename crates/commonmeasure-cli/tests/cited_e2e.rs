//! Context entry and citation on Claude Code (EDG-203), driven as the host
//! drives the shipped binary: the plugin's MCP server with
//! `--host-observations` fetches two pages from a loopback origin, a Claude
//! Code transcript records one model call that carried both results and
//! answered citing one page by its URL, and the plugin's `Stop` hook reads
//! that transcript. The relay then projects the record to a loopback
//! receiver. The transcript is a fixture in Claude Code's recorded shape;
//! the fetch results in it are the server's own answers, byte for byte.
#![cfg(unix)]

mod common;
use common::{call, payload, write_policy};

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

use commonmeasure_http::{Response, Server};
use commonmeasure_types::canonical::canonical_digest;
use serde_json::{Value, json};

/// A name the ruling and the relay read as public, resolved to loopback by
/// the debug binary's `COMMONMEASURE_TEST_HOSTS`.
const PUBLIC_NAME: &str = "publisher.test";
const MCP_SESSION: &str = "mcp-session";
const HOST_SESSION: &str = "host-session";
const CITED_TEXT: &str = "The energy price cap is set every quarter.";
const OTHER_TEXT: &str = "Standing charges vary by region.";
const ANSWER: &str = "The cap changes quarterly";

fn hook(home: &Path, event: &str, input: &Value) -> Output {
    let output = start_hook(home, event, input)
        .wait_with_output()
        .expect("wait");
    assert!(output.status.success(), "the hook exits zero");
    output
}

/// Start a hook with its input written, without waiting for it.
fn start_hook(home: &Path, event: &str, input: &Value) -> std::process::Child {
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["hook", event, "--host", "claude-code"])
        .env("COMMONMEASURE_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the hook should start");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    child
}

/// Fetch two URLs through the opted-in server. With `host_calls`, each
/// fetch names the host's tool call it answers, as the plugin's router does
/// for a `WebFetch`.
fn fetch_both(
    home: &Path,
    work: &Path,
    urls: [&str; 2],
    host_calls: Option<[&str; 2]>,
) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .current_dir(work)
        .args([
            "mcp",
            "--host",
            "claude-code",
            "--host-observations",
            "--session",
            MCP_SESSION,
        ])
        .env("COMMONMEASURE_HOME", home)
        .env(
            "COMMONMEASURE_TEST_HOSTS",
            format!("{PUBLIC_NAME}=127.0.0.1"),
        )
        .env_remove("AGENT_SESSION_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the server should start");
    {
        let stdin = child.stdin.as_mut().unwrap();
        for (n, url) in urls.into_iter().enumerate() {
            let mut arguments = json!({"url": url});
            if let Some(host_calls) = host_calls {
                arguments["host_call_id"] = json!(host_calls[n]);
            }
            writeln!(stdin, "{}", call("context_fetch", arguments)).unwrap();
        }
    }
    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn records(home: &Path) -> Vec<Value> {
    common::session_records(home, MCP_SESSION)
}

fn events<'a>(records: &'a [Value], event: &str) -> Vec<&'a Value> {
    records.iter().filter(|r| r["event"] == event).collect()
}

/// What the transcript holds between the fetch results and the answer.
enum Between {
    /// A message line: its type and message.
    Message(&'static str, Value),
    /// A line that is not a transcript record.
    Damage,
}

/// The direct `context_fetch` tool the plugin's server offers.
const CONTEXT_FETCH: &str = "mcp__plugin_commonmeasure_commonmeasure__context_fetch";

/// One fetch as the transcript records it: the tool the model called, its
/// input and the `tool_result` content.
struct Fetch {
    tool: &'static str,
    input: Value,
    content: Value,
}

/// The server's answers as direct `context_fetch` results.
fn direct(answers: &[Value]) -> Vec<Fetch> {
    answers
        .iter()
        .map(|answer| Fetch {
            tool: CONTEXT_FETCH,
            input: json!({}),
            content: answer["result"]["content"].clone(),
        })
        .collect()
}

/// What the host model made of a page for a `WebFetch`, as the router hands
/// the edge's text to it with the call's prompt.
const SUMMARY: &str = "The page says the cap changes every quarter.";

/// The host's identifiers for the two calls the transcript records, in
/// the order [`transcript`] numbers them.
const HOST_CALLS: [&str; 2] = ["toolu_0", "toolu_1"];

/// Two `WebFetch` calls of `urls` as Claude Code records them, each result
/// being `content`: the host model's answer about the page, which is what
/// the plugin's router returns for a routed call (`plugin/hooks/register.js`)
/// and what the native tool returns for one it answered.
fn web_fetches(urls: [&str; 2], content: &[String; 2]) -> Vec<Fetch> {
    urls.iter()
        .zip(content)
        .map(|(url, content)| Fetch {
            tool: "WebFetch",
            input: json!({"url": url, "prompt": "How often does the cap change?"}),
            content: json!(content),
        })
        .collect()
}

/// The call that asked for the fetches carried neither result; the lines
/// `between` follow the results, and the last call answered citing `cited`
/// by its URL. The answer's text, which the host's `Stop` sends as
/// `last_assistant_message`.
fn transcript(
    path: &Path,
    cwd: &Path,
    fetches: &[Fetch],
    between: &[Between],
    cited: &str,
) -> String {
    let (before, answer, text) = transcript_parts(cwd, fetches, between, cited);
    std::fs::write(path, format!("{before}{answer}")).unwrap();
    text
}

/// [`transcript`] in parts: the lines before the answer, the answer's line
/// and its text.
fn transcript_parts(
    cwd: &Path,
    fetches: &[Fetch],
    between: &[Between],
    cited: &str,
) -> (String, String, String) {
    let now = chrono::Utc::now();
    let at = |seconds: i64| {
        (now + chrono::Duration::seconds(seconds))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    };
    let line = |kind: &str, seconds: i64, message: &Value| {
        json!({"type": kind, "isSidechain": false, "timestamp": at(seconds),
               "sessionId": HOST_SESSION, "cwd": cwd, "message": message})
        .to_string()
    };
    let mut lines = vec![
        line(
            "user",
            1,
            &json!({"role": "user", "content": "How often does the cap change?"}),
        ),
        line(
            "assistant",
            2,
            &json!({"id": "msg_fetch", "model": "claude-fable-5", "role": "assistant",
            "content": fetches.iter().enumerate().map(|(n, fetch)| json!(
                {"type": "tool_use", "id": format!("toolu_{n}"), "name": fetch.tool,
                 "input": fetch.input})).collect::<Vec<_>>()}),
        ),
        line(
            "user",
            3,
            &json!({"role": "user", "content": fetches.iter().enumerate().map(|(n, fetch)| json!(
                {"type": "tool_result", "tool_use_id": format!("toolu_{n}"),
                 "content": fetch.content})).collect::<Vec<_>>()}),
        ),
    ];
    for (seconds, between) in (4..).zip(between) {
        lines.push(match between {
            Between::Message(kind, message) => line(kind, seconds, message),
            Between::Damage => {
                r#"{"type":"system","subtype":"compact_boundary",BROKEN}"#.to_owned()
            }
        });
    }
    let text = format!("{ANSWER} ([source]({cited})).");
    let answer = line(
        "assistant",
        20,
        &json!({"id": "msg_answer", "model": "claude-fable-5", "role": "assistant",
        "content": [{"type": "text", "text": text}]}),
    );
    let before: String = lines.iter().map(|line| format!("{line}\n")).collect();
    (before, format!("{answer}\n"), text)
}

/// A PDF's bytes, with the binary markers a PDF carries.
fn pdf_bytes() -> Vec<u8> {
    let mut bytes =
        b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n1 0 obj\n<< /Type /Catalog >>\nendobj\n".to_vec();
    bytes.extend(b"%%EOF\n");
    bytes
}

/// An operator home whose policy clears telemetry egress for a working
/// directory, a loopback origin under a public name, and the host session
/// started there.
struct Fixture {
    home: tempfile::TempDir,
    _work: tempfile::TempDir,
    work: std::path::PathBuf,
    transcript: std::path::PathBuf,
    base: String,
    _origin: commonmeasure_http::ServerHandle,
    /// The text of the transcript's last answer, which `Stop` names.
    answer: std::cell::RefCell<String>,
}

impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().canonicalize().unwrap();
        write_policy(
            home.path(),
            &json!({"scopes": [{"match": work, "engagement": "client",
                                "allow_telemetry_egress": true}]})
            .to_string(),
        );
        let origin = Server::bind("127.0.0.1:0")
            .unwrap()
            .spawn(|request| match request.target.as_str() {
                "/cap" => Response::text(200, CITED_TEXT),
                "/charges" => Response::text(200, OTHER_TEXT),
                "/tariffs.pdf" => {
                    let mut response = Response::new(200, pdf_bytes());
                    response.headers.set("Content-Type", "application/pdf");
                    response
                }
                _ => Response::text(404, "none"),
            })
            .unwrap();
        let base = origin.url().replace("127.0.0.1", PUBLIC_NAME);
        let fixture = Self {
            transcript: work.join("transcript.jsonl"),
            home,
            _work: dir,
            work,
            base,
            _origin: origin,
            answer: std::cell::RefCell::default(),
        };
        // Session start records the host process the hooks and the server
        // share.
        fixture.hook("session-start", "SessionStart");
        fixture
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn hook(&self, event: &str, name: &str) {
        hook(self.home(), event, &self.input(name));
    }

    /// A hook's input as Claude Code sends it: `Stop` names the text of the
    /// transcript's last answer.
    fn input(&self, name: &str) -> Value {
        let mut input = json!({"session_id": HOST_SESSION, "hook_event_name": name,
            "cwd": self.work, "transcript_path": self.transcript, "source": "startup"});
        if name == "Stop" {
            input["last_assistant_message"] = json!(*self.answer.borrow());
        }
        input
    }

    /// Fetch two URLs through the opted-in server; their answers and
    /// handles.
    fn fetch(&self, urls: [&str; 2]) -> (Vec<Value>, Vec<String>) {
        self.fetch_for(urls, None)
    }

    /// [`Self::fetch`], each fetch naming the host call it answers.
    fn fetch_for(
        &self,
        urls: [&str; 2],
        host_calls: Option<[&str; 2]>,
    ) -> (Vec<Value>, Vec<String>) {
        let answers = fetch_both(self.home(), &self.work, urls, host_calls);
        let handles = answers
            .iter()
            .map(|answer| {
                payload(answer)["acquisition_id"]
                    .as_str()
                    .expect("an opted-in fetch returns its handle")
                    .to_owned()
            })
            .collect();
        let started = events(&records(self.home()), "observations_started").len();
        assert_eq!(started, 1, "the server opted in before its first crossing");
        (answers, handles)
    }

    fn stop(&self, answers: &[Value], between: &[Between], cited: &str) {
        self.stop_after(&direct(answers), between, cited);
    }

    fn stop_after(&self, fetches: &[Fetch], between: &[Between], cited: &str) {
        *self.answer.borrow_mut() =
            transcript(&self.transcript, &self.work, fetches, between, cited);
        self.hook("stop", "Stop");
    }

    /// Run the relay to a loopback receiver: the events it sent and the
    /// bodies as they crossed.
    fn relay(&self) -> (Vec<Value>, String) {
        let bodies: Arc<Mutex<Vec<Value>>> = Arc::default();
        let captured = bodies.clone();
        let receiver = Server::bind("127.0.0.1:0")
            .unwrap()
            .spawn(move |request| {
                let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
                let count = body["events"].as_array().map(Vec::len).unwrap_or(0);
                captured.lock().unwrap().push(body);
                Response::json(
                    201,
                    &json!({"status": "ok", "events_created": count}).to_string(),
                )
            })
            .unwrap();
        std::fs::write(
            self.home().join("relay.json"),
            json!({"receiver": receiver.url()}).to_string(),
        )
        .unwrap();
        let relayed = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
            .arg("relay")
            .env("COMMONMEASURE_HOME", self.home())
            .output()
            .unwrap();
        assert!(
            relayed.status.success(),
            "{}",
            String::from_utf8_lossy(&relayed.stderr)
        );
        let bodies = bodies.lock().unwrap();
        let sent = bodies
            .iter()
            .flat_map(|body| body["events"].as_array().unwrap().clone())
            .collect();
        (sent, serde_json::to_string(&*bodies).unwrap())
    }
}

/// The content URLs of the sent events of one type, in order.
fn of(sent: &[Value], kind: &str) -> Vec<String> {
    sent.iter()
        .filter(|event| event["type"] == kind)
        .map(|event| event["content_url"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn a_turn_that_fetches_two_pages_and_cites_one_reports_one_citation() {
    let fixture = Fixture::new();
    let (cited_url, other_url) = (fixture.url("/cap"), fixture.url("/charges"));
    let (answers, handles) = fixture.fetch([&*cited_url, &*other_url]);
    fixture.stop(&answers, &[], &cited_url);

    let recorded = records(fixture.home());
    let entries = events(&recorded, "context_entered");
    let entered: Vec<&str> = entries
        .iter()
        .map(|entry| entry["payload"]["acquisition_id"].as_str().unwrap())
        .collect();
    assert_eq!(entered, handles, "one context entry per entered page");
    let generation = &entries[0]["payload"]["generation_id"];
    assert!(
        entries
            .iter()
            .all(|e| &e["payload"]["generation_id"] == generation)
    );
    let outputs = events(&recorded, "output_associated");
    assert_eq!(
        outputs.len(),
        2,
        "one output per completed assistant message"
    );
    assert_eq!(outputs[0]["payload"]["acquisition_ids"], json!([]));
    assert_eq!(outputs[1]["payload"]["generation_id"], *generation);
    assert_eq!(
        outputs[1]["payload"]["acquisition_ids"],
        json!([handles[0]])
    );
    assert_eq!(events(&recorded, "turn_completed").len(), 2);

    // A second Stop over the same transcript adds nothing.
    fixture.hook("stop", "Stop");
    assert_eq!(records(fixture.home()).len(), recorded.len());

    // The relay sends one citation, of the cited page.
    let (sent, wire) = fixture.relay();
    assert_eq!(of(&sent, "content_retrieved"), [&*cited_url, &*other_url]);
    assert_eq!(of(&sent, "content_grounded"), [&*cited_url, &*other_url]);
    assert_eq!(of(&sent, "content_cited"), [&*cited_url]);
    let citation = sent.iter().find(|e| e["type"] == "content_cited").unwrap();
    assert_eq!(citation["turn_id"], *generation);
    assert!(citation["output_id"].is_string());
    assert_eq!(citation["data"]["citation_type"], "unclassified");

    // No page text, output text or quotation leaves the machine.
    for private in [CITED_TEXT, OTHER_TEXT, ANSWER, "How often", HOST_SESSION] {
        assert!(!wire.contains(private), "{private} left the machine");
    }
    for handle in &handles {
        assert!(
            !wire.contains(handle.as_str()),
            "an acquisition handle left"
        );
    }
}

/// A line that does not parse between the results and the answer leaves
/// the request boundary unestablished: no context entry, no output and no
/// citation, and the hook still exits zero.
#[test]
fn a_damaged_transcript_line_yields_no_context_entry_and_no_citation() {
    let fixture = Fixture::new();
    let (cited_url, other_url) = (fixture.url("/cap"), fixture.url("/charges"));
    let (answers, _) = fixture.fetch([&*cited_url, &*other_url]);
    fixture.stop(&answers, &[Between::Damage], &cited_url);

    let recorded = records(fixture.home());
    assert!(events(&recorded, "context_entered").is_empty());
    assert!(events(&recorded, "output_associated").is_empty());
    assert!(events(&recorded, "turn_completed").is_empty());

    let (sent, _) = fixture.relay();
    assert_eq!(of(&sent, "content_retrieved"), [&*cited_url, &*other_url]);
    assert!(of(&sent, "content_grounded").is_empty());
    assert!(of(&sent, "content_cited").is_empty());
}

/// What the host's whole-file `Read` of the saved PDF returned, if it read
/// it at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PdfRead {
    Unread,
    /// The bytes the edge fetched, as Claude Code sends a PDF: a base64
    /// document block.
    FetchedBytes,
    /// A document of other bytes: the file was replaced after the fetch.
    OtherBytes,
    /// The note Claude Code writes in place of a result too large to send.
    Stub,
    /// A successful result with no content.
    Empty,
}

/// The stdio edge saves a PDF and names its path. The path in a request is
/// not the file, and a successful read is not its bytes: the PDF enters only
/// from the call after a whole read whose result carries the fetched bytes,
/// under the hash of those bytes, and only then is its citation projected.
#[test]
fn a_saved_pdf_enters_and_is_cited_only_after_a_read_returns_its_bytes() {
    use base64::Engine as _;
    let document = |bytes: &[u8]| {
        json!({"type": "document", "source": {"type": "base64", "media_type": "application/pdf",
               "data": base64::engine::general_purpose::STANDARD.encode(bytes)}})
    };
    for read in [
        PdfRead::Unread,
        PdfRead::FetchedBytes,
        PdfRead::OtherBytes,
        PdfRead::Stub,
        PdfRead::Empty,
    ] {
        let fixture = Fixture::new();
        let (pdf_url, page_url) = (fixture.url("/tariffs.pdf"), fixture.url("/cap"));
        let (answers, handles) = fixture.fetch([&*pdf_url, &*page_url]);
        let delivered = payload(&answers[0]);
        let path = delivered["path"].as_str().expect("the PDF is saved");
        let saved = std::fs::read(path).expect("the saved PDF");
        assert_eq!(saved, pdf_bytes(), "the edge saved the fetched bytes");
        let content = match read {
            PdfRead::Unread => None,
            PdfRead::FetchedBytes => Some(json!([document(&saved)])),
            PdfRead::OtherBytes => {
                let mut other = pdf_bytes();
                other.extend(b"% replaced\n");
                std::fs::write(path, &other).unwrap();
                Some(json!([document(&other)]))
            }
            PdfRead::Stub => Some(json!([{"type": "text", "text": format!(
                "Error: result exceeds maximum allowed tokens. Output has been saved to {path}")}])),
            PdfRead::Empty => Some(json!([])),
        };
        let between = match content {
            None => Vec::new(),
            Some(content) => vec![
                Between::Message(
                    "assistant",
                    json!({"id": "msg_read", "model": "claude-fable-5", "role": "assistant",
                    "content": [{"type": "tool_use", "id": "toolu_r", "name": "Read",
                                 "input": {"file_path": path}}]}),
                ),
                Between::Message(
                    "user",
                    json!({"role": "user", "content": [{"type": "tool_result",
                        "tool_use_id": "toolu_r", "content": content}]}),
                ),
            ],
        };
        fixture.stop(&answers, &between, &pdf_url);

        let recorded = records(fixture.home());
        let entries = events(&recorded, "context_entered");
        let pdf_entries: Vec<&&Value> = entries
            .iter()
            .filter(|e| e["payload"]["acquisition_id"] == handles[0].as_str())
            .collect();
        let outputs = events(&recorded, "output_associated");
        let answer = outputs.last().unwrap();
        assert_eq!(
            answer["payload"]["acquisition_ids"],
            json!([handles[0]]),
            "{read:?}: the answer names the PDF either way"
        );
        let (sent, _) = fixture.relay();
        assert_eq!(of(&sent, "content_retrieved"), [&*pdf_url, &*page_url]);
        if read == PdfRead::FetchedBytes {
            assert_eq!(pdf_entries.len(), 1, "entered once, in the answer's call");
            assert_eq!(
                pdf_entries[0]["payload"]["generation_id"],
                answer["payload"]["generation_id"]
            );
            assert_eq!(
                pdf_entries[0]["payload"]["representation_hash"],
                delivered["content_hash"]
            );
            assert_eq!(of(&sent, "content_cited"), [&*pdf_url]);
        } else {
            assert!(
                pdf_entries.is_empty(),
                "{read:?}: the PDF is retrieval-only"
            );
            assert!(!of(&sent, "content_grounded").contains(&pdf_url));
            assert!(
                of(&sent, "content_cited").is_empty(),
                "{read:?}: no citation"
            );
        }
    }
}

/// An output recorded without its completion boundary, as when the second
/// append failed, is finished by the next Stop: it resubmits the identical
/// output, the ingress appends the boundary, and the citation projects. A
/// further Stop is an ordinary duplicate and adds nothing.
#[test]
fn a_repeated_stop_finishes_an_output_whose_boundary_was_not_written() {
    let fixture = Fixture::new();
    let (cited_url, other_url) = (fixture.url("/cap"), fixture.url("/charges"));
    let (answers, _) = fixture.fetch([&*cited_url, &*other_url]);
    fixture.stop(&answers, &[], &cited_url);
    let whole = records(fixture.home());
    let boundaries = |records: &[Value]| -> Vec<Value> {
        events(records, "turn_completed")
            .iter()
            .map(|r| json!([r["payload"]["turn_id"], r["payload"]["output_id"]]))
            .collect()
    };
    assert_eq!(boundaries(&whole).len(), 2);

    // The log as an append failure leaves it: outputs, no boundaries.
    let log = fixture
        .home()
        .join("sessions")
        .join(format!("{MCP_SESSION}.ndjson"));
    let kept: String = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .filter(|line| !line.contains(r#""event":"turn_completed""#))
        .map(|line| format!("{line}\n"))
        .collect();
    std::fs::write(&log, kept).unwrap();
    let partial = records(fixture.home());
    assert!(boundaries(&partial).is_empty());
    assert_eq!(events(&partial, "output_associated").len(), 2);

    fixture.hook("stop", "Stop");
    let recovered = records(fixture.home());
    assert_eq!(boundaries(&recovered), boundaries(&whole));
    assert_eq!(
        recovered.len(),
        whole.len(),
        "only the boundaries were added"
    );
    assert_eq!(
        events(&recovered, "output_associated").len(),
        2,
        "no second output"
    );

    fixture.hook("stop", "Stop");
    assert_eq!(records(fixture.home()).len(), recovered.len());

    let (sent, _) = fixture.relay();
    assert_eq!(of(&sent, "content_cited"), [&*cited_url]);
}

/// A `WebFetch` the plugin's router answered through the edge (EDG-204)
/// records its context entry and citation as a direct `context_fetch` does:
/// the edge recorded the host's identifier for each call on its
/// acquisition, so the `Stop` hook attributes the model call that carried
/// the result, and the cited page projects `content_cited`. The result's
/// text names no handle.
#[test]
fn a_routed_web_fetch_records_its_context_entry_and_citation() {
    let fixture = Fixture::new();
    let (cited_url, other_url) = (fixture.url("/cap"), fixture.url("/charges"));
    let (_, handles) = fixture.fetch_for([&*cited_url, &*other_url], Some(HOST_CALLS));
    let recorded = records(fixture.home());
    let bound: Vec<&Value> = events(&recorded, "crossing_mediated")
        .iter()
        .map(|crossing| &crossing["payload"]["host_call_id"])
        .collect();
    assert_eq!(bound, [&json!(HOST_CALLS[0]), &json!(HOST_CALLS[1])]);
    let fetches = web_fetches(
        [&cited_url, &other_url],
        &[SUMMARY.to_owned(), SUMMARY.to_owned()],
    );
    fixture.stop_after(&fetches, &[], &cited_url);

    let recorded = records(fixture.home());
    let entries = events(&recorded, "context_entered");
    let entered: Vec<&str> = entries
        .iter()
        .map(|entry| entry["payload"]["acquisition_id"].as_str().unwrap())
        .collect();
    assert_eq!(entered, handles, "one context entry per routed page");
    // The representation is the routed result as the transcript records it.
    assert_eq!(
        entries[0]["payload"]["representation_hash"],
        canonical_digest(&fetches[0].content)
    );
    let outputs = events(&recorded, "output_associated");
    assert_eq!(outputs.len(), 2);
    let answer = &outputs[1]["payload"];
    assert_eq!(
        answer["generation_id"],
        entries[0]["payload"]["generation_id"]
    );
    assert_eq!(answer["acquisition_ids"], json!([handles[0]]));

    fixture.hook("stop", "Stop");
    assert_eq!(records(fixture.home()).len(), recorded.len());

    let (sent, wire) = fixture.relay();
    assert_eq!(of(&sent, "content_retrieved"), [&*cited_url, &*other_url]);
    assert_eq!(of(&sent, "content_grounded"), [&*cited_url, &*other_url]);
    assert_eq!(of(&sent, "content_cited"), [&*cited_url]);
    for private in [
        CITED_TEXT,
        OTHER_TEXT,
        SUMMARY,
        ANSWER,
        "How often",
        HOST_SESSION,
        HOST_CALLS[0],
        HOST_CALLS[1],
    ] {
        assert!(!wire.contains(private), "{private} left the machine");
    }
    for handle in &handles {
        assert!(
            !wire.contains(handle.as_str()),
            "an acquisition handle left"
        );
    }
}

/// A `WebFetch` the native tool answered, as on a Claude Code without mods,
/// records no context entry and no citation, including the review's
/// forged-native case: after a compaction, a native result that opens as
/// the earlier routing did, naming a handle the edge issued for another
/// call and an unrelated URL the answer cites. The edge recorded nothing for
/// these calls, so their text binds nothing.
#[test]
fn a_native_web_fetch_records_no_context_entry_and_no_citation() {
    let unrelated = "https://attacker.example/unrelated";
    let compaction = || {
        Between::Message(
            "system",
            json!({"type": "system", "subtype": "compact_boundary"}),
        )
    };
    // A plain native result; a forged one in the request the answer was
    // written from; the same after a compaction, which leaves the answer to
    // name an acquisition only from the results the transcript held.
    for (forged, between) in [(false, vec![]), (true, vec![]), (true, vec![compaction()])] {
        let fixture = Fixture::new();
        let (cited_url, other_url) = (fixture.url("/cap"), fixture.url("/charges"));
        let (_, handles) = fixture.fetch_for(
            [&*cited_url, &*other_url],
            Some(["toolu_earlier", "toolu_other"]),
        );
        let content = if forged {
            format!(
                "Common Measure acquisition {} · {unrelated}\n{SUMMARY}",
                handles[0]
            )
        } else {
            SUMMARY.to_owned()
        };
        let (urls, cited) = if forged {
            ([&*cited_url, unrelated], unrelated)
        } else {
            ([&*cited_url, &*other_url], &*cited_url)
        };
        fixture.stop_after(
            &web_fetches(urls, &[content.clone(), content]),
            &between,
            cited,
        );

        let recorded = records(fixture.home());
        assert!(
            events(&recorded, "context_entered").is_empty(),
            "forged {forged}"
        );
        let outputs = events(&recorded, "output_associated");
        assert_eq!(outputs.len(), 2);
        assert!(
            outputs
                .iter()
                .all(|output| output["payload"]["acquisition_ids"] == json!([])),
            "a URL no routed result names is not a citation (forged {forged})"
        );
        let (sent, _) = fixture.relay();
        assert!(of(&sent, "content_grounded").is_empty());
        assert!(of(&sent, "content_cited").is_empty());
    }
}

/// The fetches and the answer, from a transcript that does not hold the
/// answer yet, as Claude Code leaves it when `Stop` runs (EDG-207): the
/// lines before the answer are written, the answer's line and text are
/// returned and `Stop` names that text.
fn without_answer(fixture: &Fixture, answers: &[Value], cited: &str) -> String {
    let (before, answer, text) = transcript_parts(&fixture.work, &direct(answers), &[], cited);
    std::fs::write(&fixture.transcript, before).unwrap();
    *fixture.answer.borrow_mut() = text;
    answer
}

fn append(path: &Path, line: &str) {
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(line.as_bytes())
        .unwrap();
}

/// The `turn_completed` boundaries of the hook's own log.
fn hook_boundaries(home: &Path) -> Vec<Value> {
    common::session_records(home, HOST_SESSION)
        .into_iter()
        .filter(|record| record["event"] == "turn_completed")
        .collect()
}

/// A single-turn session whose `Stop` runs before the host has written the
/// final answer to the transcript, as in the recorded `claude -p` run
/// (EDG-207). The hook reads the transcript only once it holds that answer,
/// so the answer records its context entries and its citation at its own
/// `Stop`, and nothing later is needed.
#[test]
fn a_stop_that_runs_before_the_answer_is_written_reads_it_once_written() {
    let fixture = Fixture::new();
    let (cited_url, other_url) = (fixture.url("/cap"), fixture.url("/charges"));
    let (answers, handles) = fixture.fetch([&*cited_url, &*other_url]);
    let answer = without_answer(&fixture, &answers, &cited_url);

    let stop = start_hook(fixture.home(), "stop", &fixture.input("Stop"));
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        events(&records(fixture.home()), "output_associated").is_empty(),
        "nothing is submitted before the answer is in the transcript"
    );
    append(&fixture.transcript, &answer);
    let output = stop.wait_with_output().unwrap();
    assert!(output.status.success(), "the hook exits zero");

    let recorded = records(fixture.home());
    let entered: Vec<&str> = events(&recorded, "context_entered")
        .iter()
        .map(|entry| entry["payload"]["acquisition_id"].as_str().unwrap())
        .collect();
    assert_eq!(entered, handles, "the answer's request entered both pages");
    let outputs = events(&recorded, "output_associated");
    assert_eq!(outputs.len(), 2, "the fetching call and the answer");
    assert_eq!(
        outputs[1]["payload"]["acquisition_ids"],
        json!([handles[0]])
    );
    let boundary = &hook_boundaries(fixture.home())[0]["payload"]["detail"];
    assert!(boundary.get("answer_unavailable").is_none(), "{boundary}");

    let (sent, _) = fixture.relay();
    assert_eq!(of(&sent, "content_cited"), [&*cited_url]);
}

/// A `Stop` whose answer does not reach the transcript within the bound
/// submits nothing, not even the calls before it that the transcript does
/// hold, and its boundary names the gap. The generations stay owed, so the
/// next `Stop` over the finished transcript records them all.
#[test]
fn a_stop_whose_answer_is_not_written_in_time_submits_nothing_and_names_the_gap() {
    let fixture = Fixture::new();
    let (cited_url, other_url) = (fixture.url("/cap"), fixture.url("/charges"));
    let (answers, handles) = fixture.fetch([&*cited_url, &*other_url]);
    let answer = without_answer(&fixture, &answers, &cited_url);

    let started = std::time::Instant::now();
    fixture.hook("stop", "Stop");
    assert!(started.elapsed() >= std::time::Duration::from_secs(2));
    let recorded = records(fixture.home());
    assert!(events(&recorded, "context_entered").is_empty());
    assert!(events(&recorded, "output_associated").is_empty());
    let boundaries = hook_boundaries(fixture.home());
    assert_eq!(boundaries.len(), 1);
    let gap = boundaries[0]["payload"]["detail"]["answer_unavailable"]
        .as_str()
        .unwrap();
    assert!(gap.contains("within 2000 ms"), "{gap}");

    append(&fixture.transcript, &answer);
    fixture.hook("stop", "Stop");
    let recorded = records(fixture.home());
    assert_eq!(events(&recorded, "context_entered").len(), 2);
    let outputs = events(&recorded, "output_associated");
    assert_eq!(outputs.len(), 2);
    assert_eq!(
        outputs[1]["payload"]["acquisition_ids"],
        json!([handles[0]])
    );
}

/// A transcript line of the host session, written `seconds` from now.
fn session_line(fixture: &Fixture, kind: &str, seconds: i64, message: Value) -> String {
    let at = (chrono::Utc::now() + chrono::Duration::seconds(seconds))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    format!(
        "{}\n",
        json!({"type": kind, "isSidechain": false, "timestamp": at,
               "sessionId": HOST_SESSION, "cwd": fixture.work, "message": message})
    )
}

/// Start `Stop` over a transcript that does not hold the turn's final
/// answer, and check that it submits nothing while the transcript holds
/// only `text` from before the turn's last prompt or tool result. Then write
/// `answer` and wait for the hook.
fn stop_until_written(fixture: &Fixture, answer: &str) {
    let before = records(fixture.home()).len();
    let stop = start_hook(fixture.home(), "stop", &fixture.input("Stop"));
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(
        records(fixture.home()).len(),
        before,
        "equal text before the turn's last boundary is not its final answer"
    );
    append(&fixture.transcript, answer);
    let output = stop.wait_with_output().unwrap();
    assert!(output.status.success(), "the hook exits zero");
}

/// One turn whose text before its fetch equals its eventual final answer
/// (EDG-207 review): when `Stop` runs, the transcript holds that text, the
/// fetch and its result, but not the final answer. The tool result after
/// the text means the turn's final call is not yet written, so `Stop` waits
/// for it and records its context entries and citation.
#[test]
fn text_before_a_tool_result_equal_to_the_final_answer_does_not_release_stop() {
    let fixture = Fixture::new();
    let (cited_url, other_url) = (fixture.url("/cap"), fixture.url("/charges"));
    let (answers, handles) = fixture.fetch([&*cited_url, &*other_url]);
    let answer = without_answer(&fixture, &answers, &cited_url);
    let text = fixture.answer.borrow().clone();
    let before: String = std::fs::read_to_string(&fixture.transcript)
        .unwrap()
        .lines()
        .map(|line| {
            let mut value: Value = serde_json::from_str(line).unwrap();
            if value["message"]["id"] == "msg_fetch" {
                value["message"]["content"]
                    .as_array_mut()
                    .unwrap()
                    .insert(0, json!({"type": "text", "text": text}));
            }
            format!("{value}\n")
        })
        .collect();
    std::fs::write(&fixture.transcript, before).unwrap();

    stop_until_written(&fixture, &answer);

    let recorded = records(fixture.home());
    let entered: Vec<&str> = events(&recorded, "context_entered")
        .iter()
        .map(|entry| entry["payload"]["acquisition_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        entered, handles,
        "the final answer's request entered both pages"
    );
    let outputs = events(&recorded, "output_associated");
    assert_eq!(outputs.len(), 2, "the fetching call and the final answer");
    assert_eq!(outputs[0]["payload"]["acquisition_ids"], json!([]));
    assert_eq!(
        outputs[1]["payload"]["acquisition_ids"],
        json!([handles[0]])
    );
    let boundary = &hook_boundaries(fixture.home())[0]["payload"]["detail"];
    assert!(boundary.get("answer_unavailable").is_none(), "{boundary}");

    let (sent, _) = fixture.relay();
    assert_eq!(of(&sent, "content_cited"), [&*cited_url]);
}

/// The previous turn's answer followed by a new prompt (EDG-207 review):
/// the new turn's final answer has the same text but is not yet written
/// when its `Stop` runs. The prompt after the earlier answer means it is
/// not this turn's, so `Stop` waits and records the new answer's own
/// generation, with its context entries and citation.
#[test]
fn an_earlier_turns_equal_answer_before_a_new_prompt_does_not_release_stop() {
    let fixture = Fixture::new();
    let (cited_url, other_url) = (fixture.url("/cap"), fixture.url("/charges"));
    let (answers, handles) = fixture.fetch([&*cited_url, &*other_url]);
    fixture.stop(&answers, &[], &cited_url);
    let first = records(fixture.home());
    assert_eq!(events(&first, "output_associated").len(), 2);
    let text = fixture.answer.borrow().clone();

    append(
        &fixture.transcript,
        &session_line(
            &fixture,
            "user",
            30,
            json!({"role": "user", "content": "Repeat the same source link."}),
        ),
    );
    let answer = session_line(
        &fixture,
        "assistant",
        40,
        json!({"id": "msg_answer_2", "model": "claude-fable-5", "role": "assistant",
               "content": [{"type": "text", "text": text}]}),
    );
    stop_until_written(&fixture, &answer);

    let recorded = records(fixture.home());
    let outputs = events(&recorded, "output_associated");
    assert_eq!(
        outputs.len(),
        3,
        "the second turn's answer has its own output"
    );
    let generation = &outputs[2]["payload"]["generation_id"];
    assert_ne!(*generation, outputs[1]["payload"]["generation_id"]);
    assert_eq!(
        outputs[2]["payload"]["acquisition_ids"],
        json!([handles[0]])
    );
    let entered: Vec<&str> = events(&recorded, "context_entered")
        .iter()
        .filter(|entry| entry["payload"]["generation_id"] == *generation)
        .map(|entry| entry["payload"]["acquisition_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        entered, handles,
        "the second answer's request held both pages"
    );
    let boundaries = hook_boundaries(fixture.home());
    assert_eq!(boundaries.len(), 2);
    let detail = &boundaries[1]["payload"]["detail"];
    assert!(detail.get("answer_unavailable").is_none(), "{detail}");
}
