//! The index against real log files on disk: the NDJSON session logs are the
//! contract (`docs/contracts/session-evidence.md`), and these tests write that
//! shape and assert what the index derives from it.

use std::path::Path;

use commonmeasure_console::{Attribution, Store};
use serde_json::Value;

fn crossing(event: &str, mode: &str, url: &str, grounded: bool, extra: &str) -> String {
    format!(
        r#"{{"seq":1,"timestamp":"2026-08-01T10:00:00.000Z","event":"{event}","payload":{{"session_id":"s","timestamp":"2026-08-01T10:00:00Z","mode":"{mode}","host":"claude-code","url":"{url}","host_name":"{host}","grounded":{grounded},"licence":{{"state":"unknown"}}{extra}}}}}"#,
        host = url::host(url),
    )
}

mod url {
    pub fn host(url: &str) -> &str {
        url.trim_start_matches("https://")
            .split('/')
            .next()
            .unwrap_or_default()
    }
}

fn write_session(dir: &Path, session: &str, lines: &[String]) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join(format!("{session}.ndjson")),
        lines.join("\n") + "\n",
    )
    .unwrap();
}

/// A log written byte for byte, for the shapes a `&[String]` cannot express:
/// bytes that are not UTF-8, and a final line with no newline behind it.
fn write_session_bytes(dir: &Path, session: &str, bytes: &[u8]) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(format!("{session}.ndjson")), bytes).unwrap();
}

#[test]
fn grades_stay_apart_in_every_aggregate() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    write_session(
        &sessions,
        "s-1",
        &[
            crossing(
                "crossing_observed",
                "observed",
                "https://a.example/page",
                true,
                "",
            ),
            crossing(
                "crossing_reconstructed",
                "reconstructed",
                "https://a.example/page/",
                true,
                r#","derived_from":"t.jsonl""#,
            ),
            crossing(
                "crossing_mediated",
                "mediated",
                "https://b.example/x",
                true,
                r#","breach":"Host b.example is outside the allowed-host list.""#,
            ),
            crossing(
                "crossing_refused",
                "mediated",
                "https://c.example/y",
                false,
                r#","refusal":"denied host""#,
            ),
        ],
    );

    let mut store = Store::open(&home.path().join("telemetry.db")).unwrap();
    let report = store.ingest_sessions(&sessions).unwrap();
    assert_eq!(report.new_records, 4);

    let overview = store.sessions(&Attribution::none(), None).unwrap();
    let session = &overview.as_array().unwrap()[0];
    assert_eq!(session["observed"], 1);
    assert_eq!(session["mediated"], 1);
    assert_eq!(session["refused"], 1);
    assert_eq!(session["reconstructed"], 1);
    assert_eq!(session["breached"], 1);
    assert_eq!(
        session["grounded_witnessed"], 2,
        "observed and mediated grounding was witnessed"
    );
    assert_eq!(
        session["grounded_reconstructed"], 1,
        "transcript-claimed grounding stays its own figure"
    );

    // The content view folds /page and /page/ into one URL and still keeps
    // the grades apart within the row.
    let content = store.content(10, &Attribution::none(), None).unwrap();
    let page = content
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["url"] == "https://a.example/page")
        .expect("trailing slash folded into one row");
    assert_eq!(page["witnessed"], 1);
    assert_eq!(page["reconstructed"], 1);
    assert_eq!(page["grounded_witnessed"], 1);
    assert_eq!(page["grounded_reconstructed"], 1);
}

/// Ingest is incremental and idempotent: lines already indexed are not
/// re-read, and re-ingesting the same files adds nothing.
#[test]
fn ingest_is_incremental_and_idempotent() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    write_session(
        &sessions,
        "s-1",
        &[crossing(
            "crossing_observed",
            "observed",
            "https://a.example/1",
            true,
            "",
        )],
    );

    let mut store = Store::open(&home.path().join("telemetry.db")).unwrap();
    assert_eq!(store.ingest_sessions(&sessions).unwrap().new_records, 1);
    assert_eq!(store.ingest_sessions(&sessions).unwrap().new_records, 0);

    // The session continues: only the new line is read.
    let path = sessions.join("s-1.ndjson");
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str(&crossing(
        "crossing_observed",
        "observed",
        "https://a.example/2",
        true,
        "",
    ));
    text.push('\n');
    std::fs::write(&path, text).unwrap();
    assert_eq!(store.ingest_sessions(&sessions).unwrap().new_records, 1);
}

/// A rebuilt store (the logs were deleted and re-imported) is noticed and
/// re-indexed rather than merged with rows describing files that no longer
/// exist.
#[test]
fn a_rebuilt_log_replaces_its_index_rows() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    write_session(
        &sessions,
        "s-1",
        &[
            crossing(
                "crossing_observed",
                "observed",
                "https://a.example/1",
                true,
                "",
            ),
            crossing(
                "crossing_observed",
                "observed",
                "https://a.example/2",
                true,
                "",
            ),
        ],
    );
    let mut store = Store::open(&home.path().join("telemetry.db")).unwrap();
    assert_eq!(store.ingest_sessions(&sessions).unwrap().new_records, 2);

    // The log is rebuilt with different content.
    write_session(
        &sessions,
        "s-1",
        &[crossing(
            "crossing_mediated",
            "mediated",
            "https://b.example/x",
            true,
            "",
        )],
    );
    let report = store.ingest_sessions(&sessions).unwrap();
    assert_eq!(report.rebuilt_files, 1);

    let overview = store.sessions(&Attribution::none(), None).unwrap();
    let session = &overview.as_array().unwrap()[0];
    assert_eq!(session["observed"], 0, "the old rows are gone");
    assert_eq!(session["mediated"], 1);
}

/// A line the index cannot parse is counted and kept, never skipped: an index
/// that silently holds less than the log understates every count it serves.
#[test]
fn unreadable_lines_are_counted_not_skipped() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    write_session(
        &sessions,
        "s-1",
        &[
            "{not json at all".to_owned(),
            crossing(
                "crossing_observed",
                "observed",
                "https://a.example/1",
                true,
                "",
            ),
        ],
    );
    let mut store = Store::open(&home.path().join("telemetry.db")).unwrap();
    store.ingest_sessions(&sessions).unwrap();

    let status = store
        .status(
            commonmeasure_relay::egress_report(home.path()),
            &Attribution::none(),
            None,
        )
        .unwrap();
    assert_eq!(status["records"], 2);
    assert_eq!(status["unreadable_lines"], 1);
    assert_eq!(
        status["egress"]["receiver"],
        Value::Null,
        "no receiver is configured and none may be implied"
    );
    assert_eq!(status["egress"]["delivered"], 0);
}

/// The same discipline for the sibling case: a line that is not UTF-8 is
/// unparseable exactly like a line of invalid JSON, so it is kept as
/// `unreadable` too. It must never cost the valid lines beside it — in its own
/// file or in any other — because ingest runs per request, so a batch that
/// fails on one byte answers every route with an error for as long as the byte
/// is on disk.
#[test]
fn a_line_that_is_not_utf8_is_kept_like_any_other_unreadable_line() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    let good = crossing(
        "crossing_observed",
        "observed",
        "https://a.example/1",
        true,
        "",
    );
    let mut log = vec![0xff, 0xfe, b'\n'];
    log.extend_from_slice(good.as_bytes());
    log.push(b'\n');
    write_session_bytes(&sessions, "s-1", &log);
    write_session(
        &sessions,
        "s-2",
        &[crossing(
            "crossing_mediated",
            "mediated",
            "https://b.example/2",
            true,
            "",
        )],
    );

    let mut store = Store::open(&home.path().join("telemetry.db")).unwrap();
    store
        .ingest_sessions(&sessions)
        .expect("one byte that is not UTF-8 must not fail the batch");

    let status = store
        .status(
            commonmeasure_relay::egress_report(home.path()),
            &Attribution::none(),
            None,
        )
        .unwrap();
    assert_eq!(status["records"], 3);
    assert_eq!(status["unreadable_lines"], 1);
    let overview = store.sessions(&Attribution::none(), None).unwrap();
    let session_of = |id: &str| {
        overview
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["session_id"] == id)
            .unwrap_or_else(|| panic!("{id} was indexed"))
            .clone()
    };
    assert_eq!(
        session_of("s-1")["observed"],
        1,
        "the valid line beside the bad one is indexed"
    );
    assert_eq!(
        session_of("s-1")["unreadable"],
        1,
        "and the bad one is kept, not skipped"
    );
    assert_eq!(
        session_of("s-2")["mediated"],
        1,
        "a bad byte in one file does not cost another file its lines"
    );
}

/// A read that does fail names the file it failed on: an operator told only
/// that a stream went wrong cannot find the culprit among their logs.
#[test]
fn a_log_that_cannot_be_read_names_itself_in_the_error() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    // A directory wearing a log's name: opens like a file, fails on read.
    std::fs::create_dir_all(sessions.join("wedged.ndjson")).unwrap();
    let mut store = Store::open(&home.path().join("telemetry.db")).unwrap();

    let error = format!("{:#}", store.ingest_sessions(&sessions).unwrap_err());
    assert!(
        error.contains("wedged.ndjson"),
        "the failure must name its culprit, got: {error}"
    );
}

/// A final line the writer has not finished is not a record yet. Indexing the
/// fragment would fix the truncation in the index forever — the watermark is
/// durable and nothing revisits a line below it — so the reader stops short of
/// it and reads it whole on the next pass.
#[test]
fn a_torn_final_line_waits_for_its_newline() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    let line = |n: u32| {
        crossing(
            "crossing_observed",
            "observed",
            &format!("https://a.example/{n}"),
            true,
            "",
        )
    };
    let log_of = |lines: &[String]| {
        let mut bytes = Vec::new();
        for line in lines {
            bytes.extend_from_slice(line.as_bytes());
            bytes.push(b'\n');
        }
        bytes
    };

    // The writer is mid-line: two records are complete, the third is a stub.
    let third = line(3);
    let mut log = log_of(&[line(1), line(2)]);
    log.extend_from_slice(&third.as_bytes()[..third.len() / 2]);
    write_session_bytes(&sessions, "s-1", &log);

    let mut store = Store::open(&home.path().join("telemetry.db")).unwrap();
    assert_eq!(store.ingest_sessions(&sessions).unwrap().new_records, 2);
    let status_of = |store: &Store| {
        store
            .status(
                commonmeasure_relay::egress_report(home.path()),
                &Attribution::none(),
                None,
            )
            .unwrap()
    };
    let torn = status_of(&store);
    assert_eq!(torn["records"], 2, "the fragment is not a record yet");
    assert_eq!(torn["unreadable_lines"], 0);

    // The writer finishes the line and appends the next one.
    write_session_bytes(
        &sessions,
        "s-1",
        &log_of(&[line(1), line(2), line(3), line(4)]),
    );
    store.ingest_sessions(&sessions).unwrap();
    let settled = status_of(&store);
    assert_eq!(settled["records"], 4);
    assert_eq!(
        settled["unreadable_lines"], 0,
        "a completed line leaves no permanent unreadable behind"
    );

    let records = store.session_records("s-1").unwrap().unwrap();
    let urls: Vec<&str> = records
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["payload"]["url"].as_str().unwrap_or("unreadable"))
        .collect();
    assert_eq!(
        urls,
        [
            "https://a.example/1",
            "https://a.example/2",
            "https://a.example/3",
            "https://a.example/4",
        ],
        "the index holds the log's real records, not the fragment it saw first"
    );
}

/// The session view and the content view read one event list. An event this
/// runtime does not define is evidence of nothing in either — least of all of
/// the strongest grade — and a refused crossing grounds nothing, because a
/// crossing that did not happen cannot have been grounded.
#[test]
fn an_undefined_crossing_event_is_evidence_in_neither_view() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    write_session(
        &sessions,
        "s-1",
        &[
            crossing(
                "crossing_teleported",
                "observed",
                "https://a.example/page",
                true,
                "",
            ),
            crossing(
                "crossing_refused",
                "mediated",
                "https://a.example/page",
                true,
                r#","refusal":"denied host""#,
            ),
        ],
    );
    let mut store = Store::open(&home.path().join("telemetry.db")).unwrap();
    store.ingest_sessions(&sessions).unwrap();

    let overview = store.sessions(&Attribution::none(), None).unwrap();
    let session = &overview.as_array().unwrap()[0];
    assert_eq!(session["observed"], 0);
    assert_eq!(session["mediated"], 0);
    assert_eq!(session["refused"], 1);
    assert_eq!(session["grounded_witnessed"], 0);
    assert_eq!(session["grounded_reconstructed"], 0);

    let content = store.content(10, &Attribution::none(), None).unwrap();
    let row = &content.as_array().unwrap()[0];
    assert_eq!(
        row["witnessed"], 0,
        "an event the runtime does not define is not a witnessed crossing"
    );
    assert_eq!(row["reconstructed"], 0);
    assert_eq!(row["refused"], 1);
    assert_eq!(
        row["grounded_witnessed"], 0,
        "neither an undefined event nor a refusal grounds anything"
    );
}

/// The rows go when the file does. An aggregate over evidence that was deleted
/// cannot be re-derived by reading the same files by hand, which is the only
/// thing that makes it an aggregate rather than an assertion.
#[test]
fn a_deleted_log_takes_its_rows_with_it() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    for (session, url) in [
        ("s-1", "https://a.example/1"),
        ("s-2", "https://b.example/2"),
    ] {
        write_session(
            &sessions,
            session,
            &[crossing("crossing_observed", "observed", url, true, "")],
        );
    }
    let mut store = Store::open(&home.path().join("telemetry.db")).unwrap();
    store.ingest_sessions(&sessions).unwrap();
    let status_of = |store: &Store| {
        store
            .status(
                commonmeasure_relay::egress_report(home.path()),
                &Attribution::none(),
                None,
            )
            .unwrap()
    };
    assert_eq!(status_of(&store)["files"], 2);
    assert_eq!(status_of(&store)["records"], 2);

    std::fs::remove_file(sessions.join("s-2.ndjson")).unwrap();
    store.ingest_sessions(&sessions).unwrap();

    let status = status_of(&store);
    assert_eq!(status["files"], 1);
    assert_eq!(status["records"], 1);
    let overview = store.sessions(&Attribution::none(), None).unwrap();
    let listed: Vec<&str> = overview
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["session_id"].as_str().unwrap())
        .collect();
    assert_eq!(listed, ["s-1"], "the deleted log's session is gone with it");
    assert!(
        store.session_records("s-2").unwrap().is_none(),
        "and nothing serves its records"
    );
}

/// The engagement-scoping acceptance, end to end at the store: two sessions
/// from different directories appear under different engagements, from one
/// store, with `unattributed` shown for a third — and editing a rule
/// re-attributes them provably without any evidence file changing.
#[test]
fn one_store_many_engagements_and_rules_reattribute_without_touching_evidence() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    write_session(
        &sessions,
        "s-ozone",
        &[crossing(
            "crossing_observed",
            "observed",
            "https://a.example/1",
            true,
            r#","cwd":"/home/op/code/ozone/api""#,
        )],
    );
    write_session(
        &sessions,
        "s-tessera",
        &[crossing(
            "crossing_mediated",
            "mediated",
            "https://b.example/2",
            true,
            r#","cwd":"/home/op/code/tessera-site""#,
        )],
    );
    write_session(
        &sessions,
        "s-elsewhere",
        &[crossing(
            "crossing_observed",
            "observed",
            "https://c.example/3",
            false,
            r#","cwd":"/home/op/exploration""#,
        )],
    );
    std::fs::write(
        home.path().join("attribution.json"),
        r#"{"rules":[
            {"match":"code/ozone","engagement":"ozone"},
            {"match":"tessera","engagement":"tessera"}
        ]}"#,
    )
    .unwrap();

    // Snapshot every evidence log byte for byte — stronger than a hash, and
    // provable by diff if it ever fails.
    let snapshot_logs = || {
        let mut names: Vec<_> = std::fs::read_dir(&sessions)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        names.sort();
        names
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect::<Vec<_>>()
    };
    let evidence_before = snapshot_logs();

    let mut store = Store::open(&home.path().join("telemetry.db")).unwrap();
    store.ingest_sessions(&sessions).unwrap();

    let attribution = Attribution::load(home.path()).unwrap();
    let engagement_of = |overview: &serde_json::Value, id: &str| {
        overview
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["session_id"] == id)
            .unwrap()["engagement"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let overview = store.sessions(&attribution, None).unwrap();
    assert_eq!(engagement_of(&overview, "s-ozone"), "ozone");
    assert_eq!(engagement_of(&overview, "s-tessera"), "tessera");
    assert_eq!(
        engagement_of(&overview, "s-elsewhere"),
        "unattributed",
        "work no rule names is shown plainly, not hidden"
    );

    // The filter is a projection over the one store.
    let filtered = store.sessions(&attribution, Some("ozone")).unwrap();
    assert_eq!(filtered.as_array().unwrap().len(), 1);
    let content = store.content(10, &attribution, Some("tessera")).unwrap();
    let rows = content.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["url"], "https://b.example/2");
    assert_eq!(rows[0]["engagements"][0], "tessera");
    let status = store
        .status(
            commonmeasure_relay::egress_report(home.path()),
            &attribution,
            None,
        )
        .unwrap();
    let engagements: Vec<&str> = status["engagements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["engagement"].as_str().unwrap())
        .collect();
    assert_eq!(engagements, ["ozone", "tessera", "unattributed"]);

    // Editing a rule re-attributes history on the next read, and the evidence
    // logs are untouched: attribution is a projection, never a rewrite.
    std::fs::write(
        home.path().join("attribution.json"),
        r#"{"rules":[
            {"match":"code/ozone","engagement":"advance"},
            {"match":"tessera","engagement":"advance"}
        ]}"#,
    )
    .unwrap();
    let edited = Attribution::load(home.path()).unwrap();
    let overview = store.sessions(&edited, None).unwrap();
    assert_eq!(engagement_of(&overview, "s-ozone"), "advance");
    assert_eq!(engagement_of(&overview, "s-tessera"), "advance");
    assert_eq!(
        snapshot_logs(),
        evidence_before,
        "re-attribution must not rewrite a byte of evidence"
    );
}

/// The engagement filter narrows a projection, never the record: whole-store
/// totals in the status answer stay whole-store while `engagements` is the
/// only block the filter touches.
#[test]
fn a_filtered_status_keeps_whole_store_totals() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    write_session(
        &sessions,
        "s-ozone",
        &[crossing(
            "crossing_observed",
            "observed",
            "https://a.example/1",
            true,
            r#","cwd":"/home/op/code/ozone/api""#,
        )],
    );
    write_session(
        &sessions,
        "s-tessera",
        &[crossing(
            "crossing_mediated",
            "mediated",
            "https://b.example/2",
            true,
            r#","cwd":"/home/op/code/tessera-site""#,
        )],
    );
    std::fs::write(
        home.path().join("attribution.json"),
        r#"{"rules":[
            {"match":"code/ozone","engagement":"ozone"},
            {"match":"tessera","engagement":"tessera"}
        ]}"#,
    )
    .unwrap();
    let mut store = Store::open(&home.path().join("telemetry.db")).unwrap();
    store.ingest_sessions(&sessions).unwrap();
    let attribution = Attribution::load(home.path()).unwrap();

    let whole = store
        .status(
            commonmeasure_relay::egress_report(home.path()),
            &attribution,
            None,
        )
        .unwrap();
    let filtered = store
        .status(
            commonmeasure_relay::egress_report(home.path()),
            &attribution,
            Some("ozone"),
        )
        .unwrap();
    assert_eq!(filtered["files"], whole["files"]);
    assert_eq!(filtered["records"], whole["records"]);
    assert_eq!(filtered["unreadable_lines"], whole["unreadable_lines"]);
    assert_eq!(whole["engagements"].as_array().unwrap().len(), 2);
    assert_eq!(
        filtered["engagements"],
        serde_json::json!([{
            "engagement": "ozone",
            "sessions": 1,
            "witnessed": 1,
            "reconstructed": 0,
            "refused": 0,
        }]),
        "only the engagements projection narrows, and grades stay apart in it"
    );
}

/// A session split evenly across two engagements resolves to the
/// alphabetically first, so the tie is deterministic rather than
/// ingest-order-dependent — and a URL read under two engagements belongs to
/// both content projections, because the content view attributes per
/// crossing, not per session.
#[test]
fn an_engagement_tie_is_alphabetical_and_a_shared_url_lists_both() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    write_session(
        &sessions,
        "s-split",
        &[
            crossing(
                "crossing_observed",
                "observed",
                "https://shared.example/page",
                true,
                r#","cwd":"/home/op/code/zebra/api""#,
            ),
            crossing(
                "crossing_observed",
                "observed",
                "https://shared.example/page",
                true,
                r#","cwd":"/home/op/code/aardvark/api""#,
            ),
        ],
    );
    std::fs::write(
        home.path().join("attribution.json"),
        r#"{"rules":[
            {"match":"code/zebra","engagement":"zebra"},
            {"match":"code/aardvark","engagement":"aardvark"}
        ]}"#,
    )
    .unwrap();
    let mut store = Store::open(&home.path().join("telemetry.db")).unwrap();
    store.ingest_sessions(&sessions).unwrap();
    let attribution = Attribution::load(home.path()).unwrap();

    let overview = store.sessions(&attribution, None).unwrap();
    assert_eq!(
        overview.as_array().unwrap()[0]["engagement"],
        "aardvark",
        "a one-one tie resolves alphabetically, not by ingest order"
    );

    let content = store.content(10, &attribution, None).unwrap();
    let row = &content.as_array().unwrap()[0];
    let mut engagements: Vec<&str> = row["engagements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|engagement| engagement.as_str().unwrap())
        .collect();
    engagements.sort_unstable();
    assert_eq!(
        engagements,
        ["aardvark", "zebra"],
        "the content view attributes per crossing, so both engagements own the URL"
    );
}

/// With no rule file present nothing changes but one honest label: every
/// aggregate carries `unattributed`, and no filter, count or grade moves.
#[test]
fn no_rule_file_leaves_the_defaults_standing() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    write_session(
        &sessions,
        "s-1",
        &[crossing(
            "crossing_observed",
            "observed",
            "https://a.example/1",
            true,
            r#","cwd":"/home/op/code/ozone""#,
        )],
    );
    let mut store = Store::open(&home.path().join("telemetry.db")).unwrap();
    store.ingest_sessions(&sessions).unwrap();

    let attribution = Attribution::load(home.path()).unwrap();
    let overview = store.sessions(&attribution, None).unwrap();
    let session = &overview.as_array().unwrap()[0];
    assert_eq!(session["engagement"], "unattributed");
    assert_eq!(session["observed"], 1);
    let status = store
        .status(
            commonmeasure_relay::egress_report(home.path()),
            &attribution,
            None,
        )
        .unwrap();
    assert_eq!(status["engagements"][0]["engagement"], "unattributed");
    assert_eq!(status["engagements"][0]["sessions"], 1);
}

/// The internal-use view: only crossings the capture path stamped `internal`
/// appear, folded by URL with the grades apart, the footprint a lower bound,
/// and both ends of the recorded span carried. The stamp is the filter — a
/// public URL beside them stays out of this view no matter how often it was
/// crossed.
#[test]
fn internal_use_folds_stamped_crossings_and_leaves_public_traffic_out() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    let internal_observed = r#"{"seq":1,"timestamp":"2026-08-01T10:00:00.000Z","event":"crossing_observed","payload":{"session_id":"s","timestamp":"2026-08-01T10:00:00Z","mode":"observed","host":"claude-code","cwd":"/home/op/code/ozone","url":"https://rag.corp.internal/kb/leave","host_name":"rag.corp.internal","internal":true,"grounded":true,"estimated_tokens":100,"token_basis":"characters/4","licence":{"state":"unknown"}}}"#.to_owned();
    let internal_reconstructed = r#"{"seq":2,"timestamp":"2026-08-02T11:00:00.000Z","event":"crossing_reconstructed","payload":{"session_id":"s","timestamp":"2026-08-02T11:00:00Z","mode":"reconstructed","host":"claude-code","cwd":"/home/op/code/ozone","url":"https://rag.corp.internal/kb/leave/","host_name":"rag.corp.internal","internal":true,"grounded":false,"derived_from":"t.jsonl","licence":{"state":"unknown"}}}"#.to_owned();
    let public = crossing(
        "crossing_observed",
        "observed",
        "https://a.example/page",
        true,
        r#","cwd":"/home/op/code/ozone""#,
    );
    // Refused after the fetch: it records the estimate of the text a screen
    // withheld, which never entered context.
    let internal_refused = r#"{"seq":4,"timestamp":"2026-08-01T12:00:00.000Z","event":"crossing_refused","payload":{"session_id":"s","timestamp":"2026-08-01T12:00:00Z","mode":"mediated","host":"claude-code","cwd":"/home/op/code/ozone","url":"https://rag.corp.internal/kb/leave","host_name":"rag.corp.internal","internal":true,"grounded":false,"refusal":"screened","estimated_tokens":500,"token_basis":"characters/4","licence":{"state":"unknown"}}}"#.to_owned();
    let other_engagement = r#"{"seq":3,"timestamp":"2026-08-03T09:00:00.000Z","event":"crossing_mediated","payload":{"session_id":"s","timestamp":"2026-08-03T09:00:00Z","mode":"mediated","host":"claude-code","cwd":"/home/op/code/acme","url":"file:///corp/kb/handbook","host_name":"","internal":true,"grounded":true,"licence":{"state":"unknown"}}}"#.to_owned();
    write_session(
        &sessions,
        "s-1",
        &[
            internal_observed,
            internal_reconstructed,
            internal_refused,
            public,
            other_engagement,
        ],
    );
    std::fs::write(
        home.path().join("attribution.json"),
        r#"{"rules": [
            {"match": "code/ozone", "engagement": "ozone"},
            {"match": "code/acme", "engagement": "acme"}
        ]}"#,
    )
    .unwrap();

    let mut store = Store::open(&home.path().join("telemetry.db")).unwrap();
    store.ingest_sessions(&sessions).unwrap();
    let attribution = Attribution::load(home.path()).unwrap();

    let rows = store.internal_use(10, &attribution, None).unwrap();
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 2, "the public URL stays out of this view");
    assert!(
        rows.iter()
            .all(|row| row["url"] != "https://a.example/page"),
        "a public crossing appeared in the internal-use view"
    );

    // Trailing slash folded; grades apart; each footprint a lower bound with
    // its own estimate-less crossings counted beside it; both ends of the
    // span held.
    let rag = rows
        .iter()
        .find(|row| row["url"] == "https://rag.corp.internal/kb/leave")
        .expect("the folded internal path");
    assert_eq!(rag["witnessed"], 1);
    assert_eq!(rag["reconstructed"], 1);
    assert_eq!(rag["refused"], 1);
    assert_eq!(rag["grounded_witnessed"], 1);
    assert_eq!(rag["estimated_tokens"], 100, "the refused text is left out");
    assert_eq!(rag["token_basis"], "characters/4");
    // The estimate-less crossing is the reconstructed one: it lowers the
    // reconstructed figure's bound, never the witnessed one.
    assert_eq!(rag["crossings_without_estimate"], 0);
    assert_eq!(rag["reconstructed_estimated_tokens"], Value::Null);
    assert_eq!(rag["reconstructed_crossings_without_estimate"], 1);
    assert_eq!(rag["engagements"], serde_json::json!(["ozone"]));
    assert!(
        rag["first_seen"]
            .as_str()
            .unwrap()
            .starts_with("2026-08-01T10:00")
    );
    assert!(
        rag["last_seen"]
            .as_str()
            .unwrap()
            .starts_with("2026-08-02T11:00")
    );

    // Attributed per crossing: the filter narrows to the engagement whose own
    // cwd the crossing recorded, and a file:// corpus path is a row like any
    // other.
    let acme = store.internal_use(10, &attribution, Some("acme")).unwrap();
    let acme = acme.as_array().unwrap();
    assert_eq!(acme.len(), 1);
    assert_eq!(acme[0]["url"], "file:///corp/kb/handbook");
    assert_eq!(acme[0]["engagements"], serde_json::json!(["acme"]));
}

/// Two internal crossings of one path, estimated on two bases: the row
/// carries each basis's figure and no combined one, and says why in a field.
/// The session and engagement rollups fold the same crossings and hold the
/// same rule. A single-basis path keeps its total.
#[test]
fn a_footprint_is_never_summed_across_bases() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    let by_characters = r#"{"seq":1,"timestamp":"2026-08-01T10:00:00.000Z","event":"crossing_observed","payload":{"session_id":"s","timestamp":"2026-08-01T10:00:00Z","mode":"observed","host":"claude-code","cwd":"/home/op/code/ozone","url":"https://rag.corp.internal/kb/leave","host_name":"rag.corp.internal","internal":true,"grounded":true,"estimated_tokens":100,"token_basis":"characters/4","licence":{"state":"unknown"}}}"#.to_owned();
    let by_tokeniser = r#"{"seq":2,"timestamp":"2026-08-01T10:05:00.000Z","event":"crossing_mediated","payload":{"session_id":"s","timestamp":"2026-08-01T10:05:00Z","mode":"mediated","host":"claude-code","cwd":"/home/op/code/ozone","url":"https://rag.corp.internal/kb/leave/","host_name":"rag.corp.internal","internal":true,"grounded":true,"estimated_tokens":40,"token_basis":"cl100k_base","licence":{"state":"unknown"}}}"#.to_owned();
    let one_basis = r#"{"seq":3,"timestamp":"2026-08-01T10:10:00.000Z","event":"crossing_observed","payload":{"session_id":"s","timestamp":"2026-08-01T10:10:00Z","mode":"observed","host":"claude-code","cwd":"/home/op/code/ozone","url":"https://rag.corp.internal/kb/expenses","host_name":"rag.corp.internal","internal":true,"grounded":true,"estimated_tokens":7,"token_basis":"characters/4","licence":{"state":"unknown"}}}"#.to_owned();
    write_session(&sessions, "s-1", &[by_characters, by_tokeniser, one_basis]);
    std::fs::write(
        home.path().join("attribution.json"),
        r#"{"rules": [{"match": "code/ozone", "engagement": "ozone"}]}"#,
    )
    .unwrap();

    let mut store = Store::open(&home.path().join("telemetry.db")).unwrap();
    store.ingest_sessions(&sessions).unwrap();
    let attribution = Attribution::load(home.path()).unwrap();

    let rows = store.internal_use(10, &attribution, None).unwrap();
    let rows = rows.as_array().unwrap();
    let mixed = rows
        .iter()
        .find(|row| row["url"] == "https://rag.corp.internal/kb/leave")
        .expect("the two-basis path");
    assert_eq!(
        mixed["estimated_tokens"],
        Value::Null,
        "140 is not a number"
    );
    assert_eq!(mixed["token_basis"], "mixed");
    assert_eq!(mixed["total_withheld"], "mixed_bases");
    assert_eq!(
        mixed["estimated_tokens_by_basis"],
        serde_json::json!({"characters/4": 100, "cl100k_base": 40})
    );
    let single = rows
        .iter()
        .find(|row| row["url"] == "https://rag.corp.internal/kb/expenses")
        .expect("the one-basis path");
    assert_eq!(single["estimated_tokens"], 7);
    assert_eq!(single["token_basis"], "characters/4");
    assert_eq!(single["total_withheld"], Value::Null);
    assert_eq!(
        single["estimated_tokens_by_basis"],
        serde_json::json!({"characters/4": 7})
    );

    // The session folded all three crossings: two bases, so no total.
    let overview = store.sessions(&attribution, None).unwrap();
    let session = &overview.as_array().unwrap()[0];
    assert_eq!(session["estimated_tokens"], Value::Null);
    assert_eq!(session["token_basis"], "mixed");
    assert_eq!(session["total_withheld"], "mixed_bases");
    assert_eq!(
        session["estimated_tokens_by_basis"],
        serde_json::json!({"characters/4": 107, "cl100k_base": 40})
    );

    // The engagement budget folds the session's per-basis figures, not a sum.
    let budgets = store.engagement_budgets(&attribution).unwrap();
    let ozone = budgets
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["engagement"] == "ozone")
        .expect("the ozone budget");
    assert_eq!(ozone["estimated_tokens"], Value::Null);
    assert_eq!(ozone["total_withheld"], "mixed_bases");
    assert_eq!(
        ozone["estimated_tokens_by_basis"],
        serde_json::json!({"characters/4": 107, "cl100k_base": 40})
    );
}

/// Witnessed and reconstructed crossings of one internal path, some with an
/// estimate and some without. The session, engagement and internal-use rows
/// each serve the witnessed figure alone under the plain names and the
/// reconstructed figure under the `reconstructed_` names. The two
/// estimate-less counts differ, so a field served from the other tally shows.
#[test]
fn every_footprint_row_keeps_the_reconstructed_figure_out_of_the_witnessed_one() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    let url = "https://rag.corp.internal/kb/leave";
    let stamp = r#","cwd":"/home/op/code/ozone","internal":true"#;
    let estimate = r#","estimated_tokens":ESTIMATE,"token_basis":"characters/4""#;
    let reconstructed = r#","derived_from":"t.jsonl""#;
    write_session(
        &sessions,
        "s-1",
        &[
            crossing(
                "crossing_observed",
                "observed",
                url,
                true,
                &format!("{stamp}{}", estimate.replace("ESTIMATE", "100")),
            ),
            crossing("crossing_mediated", "mediated", url, true, stamp),
            crossing(
                "crossing_reconstructed",
                "reconstructed",
                url,
                true,
                &format!(
                    "{stamp}{reconstructed}{}",
                    estimate.replace("ESTIMATE", "30")
                ),
            ),
            crossing(
                "crossing_reconstructed",
                "reconstructed",
                url,
                false,
                &format!("{stamp}{reconstructed}"),
            ),
            crossing(
                "crossing_reconstructed",
                "reconstructed",
                url,
                false,
                &format!("{stamp}{reconstructed}"),
            ),
        ],
    );
    std::fs::write(
        home.path().join("attribution.json"),
        r#"{"rules": [{"match": "code/ozone", "engagement": "ozone"}]}"#,
    )
    .unwrap();

    let mut store = Store::open(&home.path().join("telemetry.db")).unwrap();
    store.ingest_sessions(&sessions).unwrap();
    let attribution = Attribution::load(home.path()).unwrap();

    let sessions = store.sessions(&attribution, None).unwrap();
    let budgets = store.engagement_budgets(&attribution).unwrap();
    let internal = store.internal_use(10, &attribution, None).unwrap();
    let rows = [
        ("session", &sessions.as_array().unwrap()[0]),
        (
            "engagement",
            budgets
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["engagement"] == "ozone")
                .expect("the ozone budget"),
        ),
        ("internal-use", &internal.as_array().unwrap()[0]),
    ];
    for (name, row) in rows {
        assert_eq!(row["estimated_tokens"], 100, "{name}: witnessed alone");
        assert_eq!(row["token_basis"], "characters/4", "{name}");
        assert_eq!(
            row["estimated_tokens_by_basis"],
            serde_json::json!({"characters/4": 100}),
            "{name}: witnessed alone"
        );
        assert_eq!(row["crossings_without_estimate"], 1, "{name}: witnessed");
        assert_eq!(row["reconstructed_estimated_tokens"], 30, "{name}");
        assert_eq!(row["reconstructed_token_basis"], "characters/4", "{name}");
        assert_eq!(
            row["reconstructed_estimated_tokens_by_basis"],
            serde_json::json!({"characters/4": 30}),
            "{name}"
        );
        assert_eq!(
            row["reconstructed_crossings_without_estimate"], 2,
            "{name}: reconstructed"
        );
    }
}

/// A session whose crossings carried no token estimate serves no figure. Zero
/// would read as a measured footprint of nothing, which is not what happened:
/// nothing was measured (`docs/FAIL-POLICY.md` §7).
#[test]
fn a_session_with_no_token_carrying_record_serves_no_footprint() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    write_session(
        &sessions,
        "s-1",
        &[crossing(
            "crossing_observed",
            "observed",
            "https://a.example/page",
            true,
            "",
        )],
    );

    let mut store = Store::open(&home.path().join("telemetry.db")).unwrap();
    store.ingest_sessions(&sessions).unwrap();
    let attribution = Attribution::load(home.path()).unwrap();

    let overview = store.sessions(&attribution, None).unwrap();
    let session = &overview.as_array().unwrap()[0];
    assert_eq!(
        session["estimated_tokens"],
        Value::Null,
        "nothing carried an estimate, so there is no total"
    );
    assert_eq!(session["token_basis"], Value::Null);
    assert_eq!(
        session["total_withheld"],
        Value::Null,
        "nothing is withheld: there was no figure to withhold"
    );
    assert_eq!(
        session["estimated_tokens_by_basis"],
        serde_json::json!({}),
        "no basis counted anything"
    );
}

/// The content view keeps an internal-stamped crossing and marks it; a
/// crossing without the stamp is never marked. The internal-use view lists
/// the stamped path alone, and the content row carries no footprint.
#[test]
fn internal_stamped_paths_are_marked_in_the_content_view_and_public_ones_never_are() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    let internal = r#"{"seq":1,"timestamp":"2026-08-01T10:00:00.000Z","event":"crossing_observed","payload":{"session_id":"s","timestamp":"2026-08-01T10:00:00Z","mode":"observed","host":"claude-code","url":"https://rag.corp.internal/kb/leave","host_name":"rag.corp.internal","internal":true,"grounded":true,"estimated_tokens":100,"token_basis":"characters/4","licence":{"state":"unknown"}}}"#.to_owned();
    let public = crossing(
        "crossing_observed",
        "observed",
        "https://a.example/page",
        true,
        r#","estimated_tokens":12,"token_basis":"characters/4""#,
    );
    write_session(&sessions, "s-1", &[internal, public]);

    let mut store = Store::open(&home.path().join("telemetry.db")).unwrap();
    store.ingest_sessions(&sessions).unwrap();
    let attribution = Attribution::none();

    let content = store.content(10, &attribution, None).unwrap();
    let content = content.as_array().unwrap();
    assert_eq!(
        content.len(),
        2,
        "the stamped crossing is part of the record"
    );
    let stamped = content
        .iter()
        .find(|row| row["url"] == "https://rag.corp.internal/kb/leave")
        .expect("the internal path in the content view");
    assert_eq!(stamped["internal"], true);
    assert!(
        stamped.get("estimated_tokens").is_none(),
        "the content view carries no footprint"
    );
    let unstamped = content
        .iter()
        .find(|row| row["url"] == "https://a.example/page")
        .expect("the public path in the content view");
    assert_eq!(unstamped["internal"], false);

    let internal_use = store.internal_use(10, &attribution, None).unwrap();
    let internal_use = internal_use.as_array().unwrap();
    assert_eq!(internal_use.len(), 1);
    assert_eq!(internal_use[0]["url"], "https://rag.corp.internal/kb/leave");
    assert_eq!(internal_use[0]["estimated_tokens"], 100);
}

/// A crossing record with no URL is nothing a policy can judge. The forecast
/// facts count it rather than dropping it, so a forecast can say how many
/// records it left out.
#[test]
fn crossing_facts_count_the_records_they_cannot_judge() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    write_session(
        &sessions,
        "s-facts",
        &[
            crossing("crossing_observed", "observed", "https://a.example/x", true, ""),
            r#"{"seq":2,"timestamp":"2026-08-01T10:00:01.000Z","event":"crossing_observed","payload":{"session_id":"s","timestamp":"2026-08-01T10:00:01Z","mode":"observed","host":"claude-code","grounded":false,"licence":{"state":"unknown"}}}"#.to_owned(),
            crossing("crossing_reconstructed", "reconstructed", "https://b.example/y", false, ""),
        ],
    );
    let mut store = Store::open(&home.path().join("telemetry.db")).unwrap();
    store.ingest_sessions(&sessions).unwrap();
    let facts = store.crossing_facts().unwrap();
    assert_eq!(facts.not_evaluated, 1);
    let urls: Vec<&str> = facts.facts.iter().map(|fact| fact.url.as_str()).collect();
    assert_eq!(urls, ["https://a.example/x", "https://b.example/y"]);
    assert_eq!(facts.facts[0].grade, "witnessed");
    assert_eq!(facts.facts[1].grade, "reconstructed");
    assert_eq!(facts.facts[0].principal, None);
}

/// The session projection keeps the `hub_refused` an `edge_identity` record
/// carries, and has none for a record from an edge that wrote none.
#[test]
fn the_edge_identity_projection_keeps_a_hub_refusal() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    let identity = |session: &str, extra: &str| {
        format!(
            r#"{{"seq":1,"timestamp":"2026-08-01T10:00:00.000Z","event":"edge_identity","payload":{{"session_id":"{session}","host":"claude-code","timestamp":"2026-08-01T10:00:00.000Z","hub":"http://hub.example","key_id":"k-1","standing":"cleartext_hub","revoked_at":null,"revocation":null{extra}}}}}"#
        )
    };
    write_session(
        &sessions,
        "s-refused",
        &[identity(
            "s-refused",
            r#","hub_refused":"the enrolled hub URL is cleartext""#,
        )],
    );
    write_session(&sessions, "s-older", &[identity("s-older", "")]);
    let mut store = Store::open(&home.path().join("telemetry.db")).unwrap();
    store.ingest_sessions(&sessions).unwrap();
    let projected = store.host_sessions().unwrap();
    let identity_of = |id: &str| {
        projected
            .as_array()
            .unwrap()
            .iter()
            .find(|session| session["logs"][0]["session_id"] == id)
            .map(|session| session["edge_identity"].clone())
            .unwrap()
    };
    assert_eq!(
        identity_of("s-refused")["hub_refused"],
        "the enrolled hub URL is cleartext"
    );
    assert_eq!(identity_of("s-refused")["standing"], "cleartext_hub");
    assert!(identity_of("s-older")["hub_refused"].is_null());
}

/// A session record can hold the stored hub URL whole, and a home 0.4.1
/// enrolled can hold it with credentials. The log is not rewritten;
/// every projection of it names the hub by its origin alone.
#[test]
fn an_old_edge_identity_record_reads_back_as_the_hubs_origin_alone() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    write_session(
        &sessions,
        "s-old",
        &[r#"{"seq":1,"timestamp":"2026-08-01T10:00:00.000Z","event":"edge_identity","payload":{"session_id":"s-old","host":"claude-code","timestamp":"2026-08-01T10:00:00.000Z","hub":"https://user:ak_PLANTED@hub.example/","key_id":"k-1","standing":"enrolled","revoked_at":null,"revocation":null}}"#.to_owned()],
    );
    let mut store = Store::open(&home.path().join("telemetry.db")).unwrap();
    store.ingest_sessions(&sessions).unwrap();

    let projected = store.host_sessions().unwrap();
    assert!(!projected.to_string().contains("ak_PLANTED"), "{projected}");
    assert_eq!(projected[0]["edge_identity"]["hub"], "https://hub.example");

    let records = store.session_records("s-old").unwrap().unwrap();
    assert!(!records.to_string().contains("ak_PLANTED"), "{records}");
    assert_eq!(records[0]["payload"]["hub"], "https://hub.example");
    assert_eq!(records[0]["payload"]["key_id"], "k-1");
}

/// 0.4.1 built a `policy_sync` record's `policy_url` from the stored hub
/// URL, so it can hold the same credentials. The session records API names
/// that hub by its origin; a record with no policy URL keeps its null.
#[test]
fn an_old_policy_sync_record_reads_back_as_the_hubs_origin_alone() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    write_session(
        &sessions,
        "s-old",
        &[
            r#"{"seq":1,"timestamp":"2026-08-01T10:00:00.000Z","event":"policy_sync","payload":{"session_id":"s-old","timestamp":"2026-08-01T10:00:00.000Z","trigger":"session_start","policy_url":"https://user:ak_PLANTED@hub.example/api/v1/policy","outcome":"already_applied","revision":3,"digest":"sha256:ab","reason":null,"applied":null,"stale_since":null}}"#.to_owned(),
            r#"{"seq":2,"timestamp":"2026-08-01T10:00:01.000Z","event":"policy_sync","payload":{"session_id":"s-old","timestamp":"2026-08-01T10:00:01.000Z","trigger":"session_start","policy_url":null,"outcome":"unavailable","revision":null,"digest":null,"reason":"hub URL refused","applied":null,"stale_since":null}}"#.to_owned(),
        ],
    );
    let mut store = Store::open(&home.path().join("telemetry.db")).unwrap();
    store.ingest_sessions(&sessions).unwrap();

    let records = store.session_records("s-old").unwrap().unwrap();
    assert!(!records.to_string().contains("ak_PLANTED"), "{records}");
    assert_eq!(records[0]["payload"]["policy_url"], "https://hub.example");
    assert_eq!(records[0]["payload"]["revision"], 3);
    assert!(records[1]["payload"]["policy_url"].is_null(), "{records}");
}

/// An unreadable line is served as its line number in its log and its
/// length in bytes as the console holds it, without the line ending, with
/// none of its text. Where the line is not valid UTF-8 this can differ from
/// its length in the log. The number counts the empty line the ingest skips,
/// so it is the line's number in the log, not its position among records.
#[test]
fn an_unreadable_line_is_served_as_its_number_and_length() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    let good = crossing(
        "crossing_observed",
        "observed",
        "https://a.example/1",
        true,
        "",
    );
    let mut log = good.clone().into_bytes();
    log.extend_from_slice(b"\n\n{\"hub\":\"https:/op:ak_PLANTED@h\xff\r\n");
    write_session_bytes(&sessions, "s-1", &log);
    let mut store = Store::open(&home.path().join("telemetry.db")).unwrap();
    store.ingest_sessions(&sessions).unwrap();

    let records = store.session_records("s-1").unwrap().unwrap();
    assert_eq!(records[0]["payload"]["url"], "https://a.example/1");
    assert_eq!(
        records[1],
        serde_json::json!({"event": "unreadable", "line": 3, "bytes": 33}),
        "{records}"
    );
}
