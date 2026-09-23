//! `commonmeasure import` driven as a person runs it: the real binary, a real
//! transcript tree, a real session store.
//!
//! The property most of these tests defend is idempotence. Reconstructed
//! crossings are the weakest grade the store holds, and the one way to make
//! them worse is to hold each of them twice: a re-run of `import` must add
//! only what is new, and say so.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const SESSION: &str = "11111111-1111-4111-8111-111111111111";

/// A home directory holding one Claude Code transcript.
fn home_with_transcript(lines: &[&str]) -> (tempfile::TempDir, PathBuf) {
    let home = tempfile::tempdir().expect("tempdir");
    let project = home.path().join(".claude/projects/-home-someone-repo");
    std::fs::create_dir_all(&project).expect("project dir");
    let transcript = project.join(format!("{SESSION}.jsonl"));
    std::fs::write(&transcript, lines.join("\n") + "\n").expect("transcript");
    (home, transcript)
}

fn webfetch_lines(id: &str, timestamp: &str, url: &str, body: &str) -> [String; 2] {
    [
        format!(
            r#"{{"type":"assistant","timestamp":"{timestamp}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"WebFetch","input":{{"url":"{url}"}}}}]}}}}"#
        ),
        format!(
            r#"{{"type":"user","timestamp":"{timestamp}","message":{{"content":[{{"type":"tool_result","tool_use_id":"{id}","content":"{body}"}}]}}}}"#
        ),
    ]
}

fn import(home: &Path, store: &Path, dry_run: bool) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_commonmeasure"));
    command
        .arg("import")
        .env("HOME", home)
        .env("COMMONMEASURE_HOME", store);
    if dry_run {
        command.arg("--dry-run");
    }
    let output = command.output().expect("the binary should start");
    assert!(
        output.status.success(),
        "import failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The same run as [`import`], started but not waited on, so two of them can be
/// in flight at once.
fn start_import(home: &Path, store: &Path) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .arg("import")
        .env("HOME", home)
        .env("COMMONMEASURE_HOME", store)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the binary should start")
}

fn session_records(store: &Path, session: &str) -> Vec<Value> {
    let path = store.join("sessions").join(format!("{session}.ndjson"));
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("the log is NDJSON"))
        .collect()
}

/// Running `import` twice must not hold the same crossing twice. The store's
/// whole value is that a reader can trust its counts.
#[test]
fn a_second_import_adds_nothing_and_says_so() {
    let lines = webfetch_lines(
        "t1",
        "2026-07-15T09:00:00.000Z",
        "https://a.example/x",
        "body",
    );
    let (home, _) = home_with_transcript(&[&lines[0], &lines[1]]);
    let store = tempfile::tempdir().expect("store");

    let first = import(home.path(), store.path(), false);
    assert!(first.contains("1 new crossings"), "{first}");
    assert_eq!(session_records(store.path(), SESSION).len(), 1);

    let second = import(home.path(), store.path(), false);
    assert!(
        second.contains("0 new crossings") && second.contains("1 already recorded"),
        "{second}"
    );
    assert_eq!(
        session_records(store.path(), SESSION).len(),
        1,
        "a re-run must not append the crossings it already holds"
    );
}

/// A dry run reports what a real run would add, which for an already-imported
/// store is nothing. "2026 would be imported" over a store already holding all
/// 2026 is a claim the evidence does not support.
#[test]
fn a_dry_run_over_an_imported_store_reports_nothing_new() {
    let lines = webfetch_lines(
        "t1",
        "2026-07-15T09:00:00.000Z",
        "https://a.example/x",
        "body",
    );
    let (home, _) = home_with_transcript(&[&lines[0], &lines[1]]);
    let store = tempfile::tempdir().expect("store");

    import(home.path(), store.path(), false);
    let dry = import(home.path(), store.path(), true);
    assert!(
        dry.contains("0 new crossings") && dry.contains("1 already recorded"),
        "{dry}"
    );
    assert!(dry.contains("0 crossings would be imported"), "{dry}");
}

/// A transcript grows while its session continues. A later import picks up the
/// new crossings without re-recording the old ones.
#[test]
fn a_grown_transcript_imports_only_its_new_crossings() {
    let first = webfetch_lines(
        "t1",
        "2026-07-15T09:00:00.000Z",
        "https://a.example/x",
        "body",
    );
    let (home, transcript) = home_with_transcript(&[&first[0], &first[1]]);
    let store = tempfile::tempdir().expect("store");
    import(home.path(), store.path(), false);

    let later = webfetch_lines(
        "t2",
        "2026-07-15T10:00:00.000Z",
        "https://b.example/y",
        "more",
    );
    let mut text = std::fs::read_to_string(&transcript).unwrap();
    text.push_str(&format!("{}\n{}\n", later[0], later[1]));
    std::fs::write(&transcript, text).unwrap();

    let output = import(home.path(), store.path(), false);
    assert!(
        output.contains("1 new crossings") && output.contains("1 already recorded"),
        "{output}"
    );
    let records = session_records(store.path(), SESSION);
    assert_eq!(records.len(), 2);
    let urls: Vec<&str> = records
        .iter()
        .map(|record| record["payload"]["url"].as_str().unwrap())
        .collect();
    assert_eq!(urls, ["https://a.example/x", "https://b.example/y"]);
}

/// Idempotence has to hold across processes, not just across runs. A cron
/// import beside a manual one is two consoles reading one home, and if both
/// read what the log is owed before either pays it, both pay it: the log is
/// append-only, so an account of 800 crossings where 400 happened is permanent
/// — and the next import reads the doubled multiset as already recorded and
/// reports nothing wrong.
#[test]
fn two_concurrent_imports_record_each_crossing_once() {
    const FACTS: usize = 400;
    let mut lines = Vec::new();
    for fact in 0..FACTS {
        lines.extend(webfetch_lines(
            &format!("t{fact}"),
            &format!("2026-07-15T09:{:02}:{:02}.000Z", fact / 60, fact % 60),
            &format!("https://a.example/{fact}"),
            "body",
        ));
    }
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    let (home, _) = home_with_transcript(&lines);
    let store = tempfile::tempdir().expect("store");

    let racing: Vec<_> = (0..2)
        .map(|_| start_import(home.path(), store.path()))
        .collect();
    let reports: Vec<String> = racing
        .into_iter()
        .map(|child| {
            let output = child.wait_with_output().expect("the import should finish");
            assert!(
                output.status.success(),
                "import failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).into_owned()
        })
        .collect();

    let records = session_records(store.path(), SESSION);
    assert_eq!(
        records.len(),
        FACTS,
        "one record per transcript fact, whoever wrote it"
    );
    let urls: std::collections::HashSet<&str> = records
        .iter()
        .map(|record| record["payload"]["url"].as_str().expect("a url"))
        .collect();
    assert_eq!(urls.len(), FACTS, "no crossing is held twice");
    assert!(
        reports
            .iter()
            .any(|report| report.contains("400 new crossings")),
        "{reports:?}"
    );
    assert!(
        reports
            .iter()
            .any(|report| report.contains("0 new crossings")
                && report.contains("400 already recorded")),
        "the second importer must report what it found, not what it added: {reports:?}"
    );
}

/// The same URL fetched twice at different moments is two crossings, and stays
/// two: idempotence removes duplicates of records, not repetition of events.
#[test]
fn a_genuinely_repeated_fetch_is_not_collapsed() {
    let first = webfetch_lines(
        "t1",
        "2026-07-15T09:00:00.000Z",
        "https://a.example/x",
        "body",
    );
    let again = webfetch_lines(
        "t2",
        "2026-07-15T09:05:00.000Z",
        "https://a.example/x",
        "body",
    );
    let (home, _) = home_with_transcript(&[&first[0], &first[1], &again[0], &again[1]]);
    let store = tempfile::tempdir().expect("store");

    import(home.path(), store.path(), false);
    assert_eq!(session_records(store.path(), SESSION).len(), 2);

    let second = import(home.path(), store.path(), false);
    assert!(second.contains("2 already recorded"), "{second}");
    assert_eq!(session_records(store.path(), SESSION).len(), 2);
}
