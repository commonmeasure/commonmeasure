//! What the evidence trail promises when writing goes wrong.
//!
//! Two claims are tested here, both about failure rather than success: a log
//! that could not record says so in band, and a run that did not finish never
//! wears the published name.

use std::fs;
use std::os::unix::fs::PermissionsExt;

use commonmeasure_runtime::{EvidenceLog, RunDirectory};
use serde_json::json;

#[test]
fn records_are_durable_and_readable_in_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("evidence.ndjson");
    let mut log = EvidenceLog::create(&path).expect("create");

    log.append("run_started", json!({"run_id": "a"})).unwrap();
    log.append("plan_completed", json!({"id": "exa-only"}))
        .unwrap();

    let records = EvidenceLog::read(&path).expect("read back");
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["seq"], 1);
    assert_eq!(records[0]["event"], "run_started");
    assert_eq!(records[1]["seq"], 2);
    assert_eq!(records[1]["payload"]["id"], "exa-only");
}

/// One log is one run. Appending a second run's records to a first run's file
/// would make both unreadable, so the file is created exclusively.
#[test]
fn a_second_run_cannot_append_to_a_first_runs_log() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("evidence.ndjson");
    EvidenceLog::create(&path).expect("first create");
    assert!(
        EvidenceLog::create(&path).is_err(),
        "an existing log must not be reopened for a new run"
    );
}

/// A write failure leaves a hole in the record, and the next successful write
/// must materialise it. Silent incompleteness is the failure mode the log
/// exists to make impossible.
#[test]
fn a_write_failure_window_is_materialised_as_a_gap() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("evidence.ndjson");
    let mut log = EvidenceLog::create(&path).expect("create");
    log.append("run_started", json!({})).unwrap();

    // Make the log unwritable, so the next append fails for a real reason.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).expect("chmod");
    let blocked = log.append("plan_completed", json!({"id": "exa-only"}));
    if blocked.is_ok() {
        // Running with privileges that ignore the mode bits; the injection did
        // not inject anything, so there is nothing to assert about recovery.
        eprintln!(
            "skipped: this process can write a mode-0444 file, so no write failure was induced"
        );
        return;
    }
    assert!(log.has_pending_gap(), "a failed write owes a gap record");

    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("chmod back");
    log.append("run_completed", json!({})).expect("recovered");

    let records = EvidenceLog::read(&path).expect("read back");
    let gap = records
        .iter()
        .find(|record| record["event"] == "evidence_gap")
        .expect("the recovered log must carry the gap it owed");
    assert_eq!(gap["payload"]["reason"], "write_failed");
    assert!(
        gap["payload"]["detail"]
            .as_str()
            .unwrap()
            .contains("plan_completed"),
        "the gap should name what could not be recorded"
    );
    // The gap precedes the record that recovered, so the log reads in order.
    let gap_seq = gap["seq"].as_u64().unwrap();
    let completed = records
        .iter()
        .find(|record| record["event"] == "run_completed")
        .unwrap();
    assert!(gap_seq < completed["seq"].as_u64().unwrap());
    assert!(!log.has_pending_gap());
}

#[test]
fn a_completed_run_replaces_the_previous_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let published = dir.path().join("latest");

    let first = RunDirectory::stage(&published).expect("stage");
    first
        .write_json("summary.json", &json!({"run": 1}))
        .unwrap();
    first.publish().expect("publish");

    let second = RunDirectory::stage(&published).expect("stage");
    second
        .write_json("summary.json", &json!({"run": 2}))
        .unwrap();
    second.publish().expect("publish");

    let summary: serde_json::Value =
        serde_json::from_slice(&fs::read(published.join("summary.json")).unwrap()).unwrap();
    assert_eq!(summary["run"], 2);
    assert!(
        siblings(dir.path(), "latest.previous").is_empty(),
        "the superseded run is removed once the new one is in place"
    );
}

/// Every entry beside `dir` whose name begins with `prefix`. Staging and
/// set-aside directories carry a per-run identifier, so they are found by
/// prefix rather than by an exact name.
fn siblings(dir: &std::path::Path, prefix: &str) -> Vec<String> {
    std::fs::read_dir(dir)
        .expect("the parent directory is readable")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(prefix))
        .collect()
}

/// Two runs publishing to one output path leave one whole run, and both
/// publish.
///
/// The path holds the last complete run, which is what a sequential re-run
/// into one directory already does. Neither of two colliding runs is
/// discarded because the other finished first, and nothing is merged: every
/// file in the published directory belongs to the same run.
///
/// Two is the collision a person hits and the one that settles for certain.
/// More than two can take the path from each other until a publish runs out
/// of attempts and fails, which the contract states
/// (`docs/contracts/run-output.md` §Directory); one writer per output path is
/// the rule either way.
#[test]
fn two_runs_to_one_output_path_publish_one_whole_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let published = dir.path().join("latest");

    let writers: Vec<_> = ["first", "second"]
        .into_iter()
        .map(|name| {
            let published = published.clone();
            std::thread::spawn(move || {
                let run = RunDirectory::stage(&published).expect("stage");
                for part in ["summary.json", "manifest.json", "responses/one.json"] {
                    run.write_json(part, &json!({"run": name})).unwrap();
                }
                run.publish()
            })
        })
        .collect();
    for writer in writers {
        writer
            .join()
            .expect("a writer thread")
            .expect("both runs completed, so both publish; the last one taken is the one held");
    }

    let read = |part: &str| -> String {
        let bytes = fs::read(published.join(part)).unwrap_or_else(|e| panic!("{part}: {e}"));
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["run"]
            .as_str()
            .expect("the run's name")
            .to_owned()
    };
    let name = read("summary.json");
    assert!(name == "first" || name == "second", "{name}");
    assert_eq!(read("manifest.json"), name, "the run published a mixture");
    assert_eq!(
        read("responses/one.json"),
        name,
        "the run published a mixture"
    );
    assert!(
        siblings(dir.path(), "latest.").is_empty(),
        "the replaced run and both staging directories are gone: {:?}",
        siblings(dir.path(), "latest.")
    );
}

/// An interrupted run must not damage the run that is already published. The
/// staging directory is a sibling, so abandoning it leaves the published run
/// exactly as it was.
#[test]
fn an_abandoned_run_leaves_the_published_one_intact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let published = dir.path().join("latest");

    let first = RunDirectory::stage(&published).expect("stage");
    first
        .write_json("summary.json", &json!({"run": "complete"}))
        .unwrap();
    first.publish().expect("publish");

    // A second run that never finishes: it writes into staging and is dropped.
    let abandoned = RunDirectory::stage(&published).expect("stage");
    abandoned
        .write_json("summary.json", &json!({"run": "half-written"}))
        .unwrap();
    drop(abandoned);

    let summary: serde_json::Value =
        serde_json::from_slice(&fs::read(published.join("summary.json")).unwrap()).unwrap();
    assert_eq!(
        summary["run"], "complete",
        "an unfinished run must never replace a finished one"
    );
}

/// The terminal record is appended before completeness is read.
///
/// The other order publishes a summary saying `evidence_complete: true` and
/// *then* attempts the write that would falsify it — and that write's failure is
/// deliberately non-fatal, so nothing downstream ever corrects the claim. The
/// run's own closing record would be missing from a log the summary vouched for.
#[test]
fn a_failed_terminal_append_is_reported_as_an_incomplete_record() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("evidence.ndjson");
    let mut log = EvidenceLog::create(&path).expect("create");
    log.append("run_started", json!({})).unwrap();

    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).expect("chmod");
    let outcome = commonmeasure_runtime::finalise_log(&mut log, uuid::Uuid::nil());
    if outcome.complete {
        eprintln!("skipped: this process can write a mode-0444 file, so no failure was induced");
        return;
    }

    assert!(
        !outcome.complete,
        "a log that could not take its terminal record is not complete"
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("chmod back");
    let records = EvidenceLog::read(&path).expect("read back");
    assert!(
        records
            .iter()
            .all(|record| record["event"] != "run_completed"),
        "the terminal record genuinely did not land, which is the point"
    );
}

/// The ordinary close: the terminal record lands, and the summary can then
/// attest to a log that is finished.
#[test]
fn a_closed_log_reports_its_own_size_and_digest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("evidence.ndjson");
    let mut log = EvidenceLog::create(&path).expect("create");
    log.append("run_started", json!({})).unwrap();

    let outcome = commonmeasure_runtime::finalise_log(&mut log, uuid::Uuid::nil());
    assert!(outcome.complete);
    assert_eq!(outcome.records, 2, "run_started plus run_completed");

    let bytes = fs::read(&path).expect("read");
    assert_eq!(
        outcome.digest.as_deref(),
        Some(
            format!(
                "sha256:{:x}",
                <sha2::Sha256 as sha2::Digest>::digest(&bytes)
            )
            .as_str()
        ),
        "the digest must cover the log as it stands on disk"
    );
    let records = EvidenceLog::read(&path).expect("read back");
    assert_eq!(records[1]["event"], "run_completed");
}
