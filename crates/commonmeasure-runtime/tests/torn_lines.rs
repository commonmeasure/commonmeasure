//! EDG-27: a record cut short by a writer that died mid-append does not
//! make the log unreadable, and nothing else that does not parse is ever
//! skipped.
//!
//! The cross-process tests start this test binary again as a child
//! ([`child`]), so that the writers really are separate processes sharing
//! one file, as a session's hook processes and its MCP server are. A child
//! that plays a writer part-way through an append takes the log's advisory
//! lock itself and writes the record in two parts: it stands in for the
//! product's writer at the one moment a test cannot otherwise stop it in.
//! The parent always writes and reads through `EvidenceLog`.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use commonmeasure_runtime::EvidenceLog;
use commonmeasure_runtime::evidence::{
    LogPosition, Tear, TornTail, Validated, repair_torn_tail, scan,
};
use serde_json::{Value, json};

const ROLE: &str = "COMMONMEASURE_TORN_LINES_ROLE";
const LOG: &str = "COMMONMEASURE_TORN_LINES_LOG";

fn raw_append(path: &Path, bytes: &[u8]) {
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

fn events(records: &[Value]) -> Vec<&str> {
    records
        .iter()
        .map(|record| record["event"].as_str().unwrap())
        .collect()
}

/// The issue's closing test: a write that lands after a torn record. The
/// append terminates the torn line and records the gap before its own
/// record, changing no byte already written, and every reader then reads
/// the records on both sides of the tear.
#[test]
fn a_write_that_lands_after_a_torn_record_marks_it_and_the_log_reads() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.ndjson");
    let mut first = EvidenceLog::open_append(&path).unwrap();
    first.append("before", json!({"n": 1})).unwrap();
    let torn = br#"{"seq":2,"timestamp":"2026-09-29T10:00:00.000Z","event":"cros"#;
    raw_append(&path, torn);
    let before = std::fs::read(&path).unwrap();

    let error = EvidenceLog::read(&path).expect_err("an unmarked tear does not read strictly");
    assert!(
        error
            .to_string()
            .contains("line 2 ends without its newline"),
        "{error}"
    );

    let mut next = EvidenceLog::open_append(&path).unwrap();
    next.append("after", json!({"n": 2})).unwrap();
    let after = std::fs::read(&path).unwrap();
    assert!(
        after.starts_with(&before),
        "no byte already written changes"
    );
    assert_eq!(after[before.len()], b'\n', "the torn line is terminated");

    let (records, tears) = EvidenceLog::read_with(&path, TornTail::Refuse).unwrap();
    assert_eq!(events(&records), ["before", "evidence_gap", "after"]);
    assert_eq!(
        tears,
        [Tear {
            line: 2,
            bytes: torn.len() as u64,
            marked: true
        }]
    );
    let gap = &records[1]["payload"];
    assert_eq!(gap["reason"], "write_failed");
    assert_eq!(gap["torn_line_bytes"], torn.len());
    assert!(gap["from"].is_string() && gap["to"].is_string());
    assert!(
        gap["detail"]
            .as_str()
            .unwrap()
            .contains("no process was still writing it"),
        "{gap}"
    );
    assert_eq!(EvidenceLog::read(&path).unwrap(), records);
    let validated = EvidenceLog::validate(&path, LogPosition::default(), TornTail::Refuse).unwrap();
    assert!(validated.fault.is_none());
    assert_eq!(validated.end.offset, after.len() as u64);
}

/// Only a line cut short, marked by the gap record naming its length on the
/// very next line, is skipped. Everything else that does not parse stays a
/// hard error for the strict read, the relay's read and the ruling's check
/// alike, and the error names the line.
#[test]
fn a_line_that_does_not_parse_is_skipped_only_where_its_gap_record_follows_it() {
    let good = |n: u64| format!("{{\"seq\":{n},\"event\":\"e{n}\",\"payload\":{{}}}}\n");
    let gap = |bytes: usize| {
        format!(
            "{{\"seq\":9,\"event\":\"evidence_gap\",\"payload\":{{\"reason\":\"write_failed\",\"torn_line_bytes\":{bytes}}}}}\n"
        )
    };
    let torn = "{\"seq\":2,\"eve";
    let cases: Vec<(&str, String, Option<&str>)> = vec![
        (
            "marked tear",
            format!("{}{torn}\n{}{}", good(1), gap(torn.len()), good(3)),
            None,
        ),
        (
            "mid-log damage",
            format!("{}{torn}\n{}", good(1), good(3)),
            Some("line 2 does not parse"),
        ),
        (
            "damage at the end, terminated",
            format!("{}{torn}\n", good(1)),
            Some("line 2 does not parse"),
        ),
        (
            "a gap of another length",
            format!("{}{torn}\n{}", good(1), gap(torn.len() + 1)),
            Some("not the evidence_gap record of a torn line of that length"),
        ),
        (
            "a gap record after a blank line",
            format!("{}{torn}\n\n{}", good(1), gap(torn.len())),
            Some("the next line is blank"),
        ),
        (
            "two torn lines, one gap",
            format!("{}{torn}\n{torn}\n{}", good(1), gap(torn.len())),
            Some("line 2 does not parse"),
        ),
        (
            "a gap-like record of another event",
            format!(
                "{}{torn}\n{{\"event\":\"other\",\"payload\":{{\"torn_line_bytes\":{}}}}}\n",
                good(1),
                torn.len()
            ),
            Some("line 2 does not parse"),
        ),
        (
            "a gap record excuses nothing before a good line",
            format!("{torn}\n{}{}", good(1), gap(torn.len())),
            Some("line 1 does not parse"),
        ),
    ];
    for (case, text, fault) in cases {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.ndjson");
        std::fs::write(&path, &text).unwrap();
        for tail in [TornTail::Refuse, TornTail::Forecast, TornTail::Repair] {
            let read = EvidenceLog::read_with(&path, tail);
            let validated = EvidenceLog::validate(&path, LogPosition::default(), tail).unwrap();
            match fault {
                None => {
                    let (records, tears) = read.unwrap();
                    assert_eq!(events(&records), ["e1", "evidence_gap", "e3"], "{case}");
                    assert_eq!(tears.len(), 1, "{case}");
                    assert!(validated.fault.is_none(), "{case}");
                }
                Some(expected) => {
                    let error = read.expect_err(case).to_string();
                    assert!(error.contains(expected), "{case}: {error}");
                    let fault = validated.fault.expect(case).to_string();
                    assert_eq!(fault, error, "{case}: the check and the read agree");
                }
            }
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            text,
            "{case}: a terminated line is never repaired"
        );
    }
}

/// The ruling's check parses into `Validated`, which keeps nothing, and the
/// relay parses into `Value`. They accept and refuse the same lines, so the
/// ruling cannot pass a log the relay skips: lone surrogate escapes and
/// invalid UTF-8, which `serde::de::IgnoredAny` would let through, among
/// them.
#[test]
fn the_check_and_the_read_accept_exactly_the_same_lines() {
    let mut lines: Vec<Vec<u8>> = [
        r#"{"a":1}"#,
        r#"{"a":"\ud800"}"#,
        r#"{"a":"\udc00x"}"#,
        r#"{"\ud800":1}"#,
        r#"{"a":"😀"}"#,
        r#"{"a":1e400}"#,
        r#"{"a":-0.0,"b":18446744073709551616}"#,
        r#"{"a":1,"a":2}"#,
        r#"{"a":[1,2,{"b":null}]}"#,
        r#"{"a":"\x"}"#,
        r#"{"a":1} x"#,
        r#"[1,2,]"#,
        r#""a string""#,
        r#"{"a":tru}"#,
        r#"{"a":"tab	inside"}"#,
    ]
    .iter()
    .map(|line| line.as_bytes().to_vec())
    .collect();
    lines.push(b"{\"a\":\"\xff\xfe\"}".to_vec());
    lines.push(b"{\"a\":\"\xc3\"}".to_vec());
    lines.push(format!("{}1{}", "[".repeat(200), "]".repeat(200)).into_bytes());
    for line in lines {
        let value = serde_json::from_slice::<Value>(&line).is_ok();
        let validated = serde_json::from_slice::<Validated>(&line).is_ok();
        assert_eq!(value, validated, "{}", String::from_utf8_lossy(&line));
    }
}

/// A scan that resumes where a previous one ended reads only what was
/// appended since, and says what a whole read would.
#[test]
fn a_check_resumed_from_its_end_agrees_with_a_whole_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.ndjson");
    let mut log = EvidenceLog::open_append(&path).unwrap();
    log.append("a", json!({})).unwrap();
    let first = EvidenceLog::validate(&path, LogPosition::default(), TornTail::Forecast).unwrap();
    assert!(first.fault.is_none());
    assert_eq!(first.end.line, 1);
    raw_append(&path, b"{\"torn");
    let torn = EvidenceLog::validate(&path, first.end, TornTail::Forecast).unwrap();
    assert!(
        torn.fault.is_none(),
        "a dead torn tail is what the relay marks"
    );
    assert_eq!(torn.end, first.end, "an unmarked tear is not settled");
    assert!(!torn.tears[0].marked);
    log.append("b", json!({})).unwrap();
    let marked = EvidenceLog::validate(&path, torn.end, TornTail::Forecast).unwrap();
    assert!(marked.fault.is_none());
    assert_eq!(marked.end.line, 4, "a, the torn line, its gap, b");
    raw_append(&path, b"{\"damage\n");
    let damaged = EvidenceLog::validate(&path, marked.end, TornTail::Forecast).unwrap();
    let fault = damaged.fault.expect("damage").to_string();
    assert!(fault.starts_with("line 5 does not parse"), "{fault}");
    let whole = EvidenceLog::read_with(&path, TornTail::Forecast)
        .expect_err("the whole read agrees")
        .to_string();
    assert_eq!(fault, whole);
}

/// A forecast reports a dead torn tail and writes nothing; a final record
/// that is whole but for its newline is kept, as marking it only adds the
/// newline. The repair marks each once and leaves a sound log alone.
#[test]
fn a_forecast_writes_nothing_and_a_repair_marks_a_tail_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.ndjson");
    assert_eq!(repair_torn_tail(&path).unwrap(), None, "no log, no repair");
    let mut log = EvidenceLog::open_append(&path).unwrap();
    log.append("a", json!({})).unwrap();
    assert_eq!(repair_torn_tail(&path).unwrap(), None);
    let whole = br#"{"seq":2,"event":"whole","payload":{}}"#;
    raw_append(&path, whole);
    let before = std::fs::read(&path).unwrap();

    let (records, tears) = EvidenceLog::read_with(&path, TornTail::Forecast).unwrap();
    assert_eq!(events(&records), ["a", "whole"]);
    assert_eq!(
        tears,
        [Tear {
            line: 2,
            bytes: whole.len() as u64,
            marked: false
        }]
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);

    let tear = repair_torn_tail(&path).unwrap().expect("marked");
    assert_eq!(
        (tear.line, tear.bytes, tear.marked),
        (2, whole.len() as u64, true)
    );
    assert_eq!(repair_torn_tail(&path).unwrap(), None, "once");
    let (records, tears) = EvidenceLog::read_with(&path, TornTail::Refuse).unwrap();
    assert_eq!(events(&records), ["a", "whole", "evidence_gap"]);
    assert!(tears.is_empty(), "the line parses, so nothing was skipped");
}

// Cross-process tests.

/// Start this binary again as a child playing `role` on the log at `path`.
fn spawn(role: &str, path: &Path) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "child",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ROLE, role)
        .env(LOG, path)
        .spawn()
        .unwrap()
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "the child never reached {path:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn marker(log: &Path, name: &str) -> PathBuf {
    log.with_extension(name)
}

/// A record of about `size` bytes, whole or in two parts.
fn record(size: usize) -> Vec<u8> {
    let mut line = serde_json::to_vec(&json!({
        "seq": 1, "event": "child", "payload": {"text": "x".repeat(size)}
    }))
    .unwrap();
    line.push(b'\n');
    line
}

/// The child's part. Ignored when run on its own.
#[test]
#[ignore = "run by the cross-process tests as a child process"]
fn child() {
    let Ok(role) = std::env::var(ROLE) else {
        return;
    };
    let path = PathBuf::from(std::env::var(LOG).unwrap());
    match role.as_str() {
        // The product's writer, many times over.
        "appender" => {
            let mut log = EvidenceLog::open_append(&path).unwrap();
            for n in 0..200 {
                log.append(
                    "child",
                    json!({"n": n, "text": "y".repeat(n * 97 % 20_000)}),
                )
                .unwrap();
            }
        }
        // A writer part-way through an append: it holds the append lock,
        // has written half its record, and finishes when told to.
        "slow" | "killed" => {
            let mut file = std::fs::OpenOptions::new()
                .read(true)
                .append(true)
                .open(&path)
                .unwrap();
            file.lock().unwrap();
            let line = record(8_192);
            file.write_all(&line[..line.len() / 2]).unwrap();
            file.sync_all().unwrap();
            std::fs::write(marker(&path, "half"), b"").unwrap();
            if role == "killed" {
                std::thread::sleep(Duration::from_secs(600));
            }
            wait_for(&marker(&path, "go"));
            // Long enough that a waiting parent is plainly waiting.
            std::thread::sleep(Duration::from_millis(300));
            file.write_all(&line[line.len() / 2..]).unwrap();
            file.sync_all().unwrap();
        }
        other => panic!("unknown role {other}"),
    }
}

/// Two child processes and this one append to one log at once, records of
/// up to 20 KB. No line is lost, merged or torn, and no gap is recorded:
/// the lock serialises appends, so no append ever sees another's tail in
/// progress.
#[test]
fn concurrent_appenders_in_separate_processes_lose_and_tear_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.ndjson");
    std::fs::write(&path, b"").unwrap();
    let children = [spawn("appender", &path), spawn("appender", &path)];
    let mut log = EvidenceLog::open_append(&path).unwrap();
    for n in 0..200 {
        log.append(
            "parent",
            json!({"n": n, "text": "z".repeat(n * 89 % 20_000)}),
        )
        .unwrap();
    }
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    let (records, tears) = EvidenceLog::read_with(&path, TornTail::Refuse).unwrap();
    assert!(tears.is_empty());
    assert_eq!(records.len(), 600);
    assert!(
        records
            .iter()
            .all(|record| record["event"] != "evidence_gap")
    );
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.iter().filter(|byte| **byte == b'\n').count(), 600);
}

/// A writer still part-way through an append is waited for, never
/// terminated. An append started while it holds the lock with half a record
/// written waits until that record is whole and lands after it, with no
/// gap; a check started then waits too and finds nothing torn.
#[test]
fn a_live_writers_record_in_progress_is_waited_for_and_left_whole() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.ndjson");
    EvidenceLog::open_append(&path)
        .unwrap()
        .append("first", json!({}))
        .unwrap();
    let mut child = spawn("slow", &path);
    wait_for(&marker(&path, "half"));

    let appending = {
        let path = path.clone();
        std::thread::spawn(move || {
            let started = Instant::now();
            EvidenceLog::open_append(&path)
                .unwrap()
                .append("after", json!({}))
                .unwrap();
            started.elapsed()
        })
    };
    let checking = {
        let path = path.clone();
        std::thread::spawn(move || {
            EvidenceLog::validate(&path, LogPosition::default(), TornTail::Forecast).unwrap()
        })
    };
    // Both are waiting on the child now; let it finish.
    std::thread::sleep(Duration::from_millis(100));
    std::fs::write(marker(&path, "go"), b"").unwrap();
    let waited = appending.join().unwrap();
    assert!(
        waited >= Duration::from_millis(300),
        "the append waited for the writer ({waited:?})"
    );
    let checked = checking.join().unwrap();
    assert!(checked.fault.is_none() && checked.tears.is_empty());
    assert!(child.wait().unwrap().success());

    let (records, tears) = EvidenceLog::read_with(&path, TornTail::Refuse).unwrap();
    assert_eq!(events(&records), ["first", "child", "after"]);
    assert!(tears.is_empty(), "nothing was torn");
}

/// A writer killed part-way through an append leaves half a record. The
/// kernel drops its lock with it, so the next append finds no writer at
/// work, terminates the line and records the gap, and the log reads.
#[test]
fn a_record_left_by_a_killed_writer_is_marked_by_the_next_append() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.ndjson");
    EvidenceLog::open_append(&path)
        .unwrap()
        .append("first", json!({}))
        .unwrap();
    let mut child = spawn("killed", &path);
    wait_for(&marker(&path, "half"));
    child.kill().unwrap();
    child.wait().unwrap();
    let half = record(8_192).len() as u64 / 2;

    let mut log = EvidenceLog::open_append(&path).unwrap();
    log.append("after", json!({})).unwrap();
    let (records, tears) = EvidenceLog::read_with(&path, TornTail::Refuse).unwrap();
    assert_eq!(events(&records), ["first", "evidence_gap", "after"]);
    assert_eq!(records[1]["payload"]["torn_line_bytes"], half);
    assert_eq!(
        tears,
        [Tear {
            line: 2,
            bytes: half,
            marked: true
        }]
    );
    let scanned =
        scan::<Value>(&path, LogPosition::default(), TornTail::Refuse, |_, _| {}).unwrap();
    assert!(scanned.fault.is_none());
}
