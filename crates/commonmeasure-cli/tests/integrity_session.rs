//! The integrity checker's record in a session's source record.
//!
//! A synthetic release is signed here, read through the release verifier,
//! ruled on by the integrity checker and appended by the production
//! `SessionLog`; then the real binary inspects the file. The release text and
//! the reply carry recognisable words so that a test can say they are absent
//! from the log and from the dossier while the verdicts, outcomes and counts
//! survive.

use std::path::Path;
use std::process::Command;

use base64::Engine as _;
use commonmeasure_harness::session::SessionLog;
use commonmeasure_runtime::processor::{integrity, release};
use commonmeasure_types::PolicyMode;
use commonmeasure_types::canonical::sha256_digest;
use commonmeasure_types::display::{Item, Reply};
use commonmeasure_types::release::{
    KeptSpan, Passage, Release, SCHEME, Sidecar, Signed, signed_message,
};
use ed25519_dalek::{Signer as _, SigningKey};
use serde_json::{Value, json};

const URL: &str = "https://source.example/ferry";

const PARAGRAPHS: [&str; 3] = [
    "The marigold ferry left Harrowgate pier at dawn on Sunday with forty passengers aboard.",
    "Captain Ilse Varnholm said \"the marigold ferry sails at dawn without fail\" before casting off.",
    "The harbourmaster, Odo Pennick, said the crossing was calm.",
];

/// Words from the release text, the reply and the identifiers the reply
/// supplied. None of them may reach the log or the dossier.
const RECOGNISABLE: [&str; 10] = [
    "marigold",
    "Harrowgate",
    "Varnholm",
    "Pennick",
    "forty passengers",
    "without fail",
    "crossing was calm",
    "timetable surprising",
    "Quillon",
    "zephyr-lantern",
];

fn signed() -> Release {
    let text = PARAGRAPHS.join("\n\n");
    let mut passages = Vec::new();
    let mut at = 0;
    for (index, paragraph) in PARAGRAPHS.iter().enumerate() {
        passages.push(Passage {
            id: format!("p{index}"),
            start: at,
            end: at + paragraph.len(),
        });
        at += paragraph.len() + 2;
    }
    let signed = Signed {
        source: "a synthetic source".into(),
        title: "Synthetic".into(),
        revid: 1,
        url: URL.into(),
        licence: "CC BY-SA 4.0".into(),
        attribution: "Synthetic, by its authors".into(),
        terms: Vec::new(),
        display_mode: "labelled".into(),
        text_sha256: sha256_digest(text.as_bytes())
            .trim_start_matches("sha256:")
            .to_owned(),
        markers: vec![KeptSpan {
            start: passages[0].start,
            end: passages[0].end,
            label: "lead".into(),
            passage: "p0".into(),
        }],
        passages,
        released_at: "2026-10-15T00:00:00Z".into(),
    };
    let key = SigningKey::from_bytes(&[7; 32]);
    let engine = base64::engine::general_purpose::STANDARD;
    Release {
        text,
        sidecar: Sidecar {
            signature: engine.encode(key.sign(&signed_message(&signed)).to_bytes()),
            signer_public_key: engine.encode(key.verifying_key().as_bytes()),
            signature_scheme: SCHEME.into(),
            signed,
        },
    }
}

fn read(release: &Release) -> release::Read {
    let body = serde_json::to_vec(release).unwrap();
    match release::invoke(PolicyMode::Observe, URL, &body, None).1 {
        release::Outcome::Read(read) => *read,
        release::Outcome::Unreadable { reason } => panic!("unreadable: {reason}"),
    }
}

fn from_source(text: &str, passages: &[&str]) -> Item {
    Item::FromSource {
        text: text.into(),
        passages: passages.iter().map(|p| p.to_string()).collect(),
    }
}

fn reply() -> Reply {
    Reply {
        items: vec![
            from_source(PARAGRAPHS[0], &["p0"]),
            from_source(
                "Varnholm said \"the marigold ferry sails at dawn without fail\".",
                &["p1"],
            ),
            from_source(
                "Pennick said \"the marigold ferry sails at dawn without fail\".",
                &["p1"],
            ),
            Item::Own {
                text: "Readers may find the timetable surprising, notes Quillon.".into(),
            },
            Item::Quote {
                passage: "said the crossing was calm".into(),
                sentence: None,
            },
            from_source("A ship departed early.", &["zephyr-lantern-id"]),
            Item::Quote {
                passage: "p2".into(),
                sentence: None,
            },
        ],
    }
}

/// A session log holding a rendered and an abstained integrity record,
/// written by the production writer, with the records as written.
fn session(home: &Path) -> (std::path::PathBuf, Value, Value) {
    let release = signed();
    let (rendered, display) = integrity::invoke(&read(&release), URL, &reply());
    assert!(display.is_some());
    let mut tampered = release.clone();
    tampered.text = tampered.text.replace("Sunday", "Monday");
    let (abstained, display) = integrity::invoke(&read(&tampered), URL, &reply());
    assert!(display.is_none());

    let mut log = SessionLog::open(home, "integrity").unwrap();
    log.record_processor(rendered.to_value()).unwrap();
    log.record_processor(abstained.to_value()).unwrap();
    let path = log.path().to_owned();
    let lines: Vec<Value> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines.iter().all(|r| r["event"] == "processor_invoked"));
    (
        path,
        lines[0]["payload"].clone(),
        lines[1]["payload"].clone(),
    )
}

#[test]
fn the_logged_record_keeps_verdicts_outcomes_and_counts_and_no_text() {
    let home = tempfile::tempdir().unwrap();
    let (path, record, abstained) = session(home.path());
    let bytes = std::fs::read_to_string(&path).unwrap();
    for words in RECOGNISABLE {
        assert!(!bytes.contains(words), "{words:?} is in the log:\n{bytes}");
    }

    let detail = &record["detail"];
    assert_eq!(record["processor"]["name"], "integrity-checker");
    assert_eq!(detail["display_mode"], "labelled");
    assert_eq!(
        detail["counts_vector"],
        json!([1, 2, 0, 0, 2, 1, 1, 0, 1, 0, 0])
    );
    let items = detail["items"].as_array().unwrap();
    let outcomes: Vec<&str> = items
        .iter()
        .map(|i| i["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(
        outcomes,
        [
            "shown_confirmed",
            "shown_confirmed",
            "withheld_attribution",
            "shown_own",
            "quote_shown",
            "shown_unconfirmed",
            "quote_shown",
        ]
    );
    let answer = |item: usize, basis: &str| -> Value {
        items[item]["answers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["basis"] == basis)
            .cloned()
            .unwrap()
    };
    assert_eq!(answer(0, "verbatim-whole/1")["label"], "yes");
    assert_eq!(answer(0, "verbatim-whole/1")["probability"], 1.0);
    assert_eq!(answer(0, "fidelity-verifier/1")["label"], "yes");
    assert_eq!(answer(1, "attribution-lookup/1")["label"], "yes");
    assert_eq!(answer(2, "attribution-lookup/1")["label"], "no");
    assert_eq!(answer(2, "attribution-lookup/1")["probability"], 0.0);
    assert_eq!(answer(5, "verbatim-whole/1")["label"], "no");
    assert_eq!(answer(0, "keep-intact-containment/1")["label"], "no");

    // Each sentence by its hash and length; shown words by span.
    let sentence = PARAGRAPHS[0];
    assert_eq!(items[0]["text_sha256"], sha256_digest(sentence.as_bytes()));
    assert_eq!(items[0]["text_bytes"], sentence.len());
    let text = signed().text;
    let span = &items[2]["shown_span"];
    let shown =
        &text[span["start"].as_u64().unwrap() as usize..span["end"].as_u64().unwrap() as usize];
    assert_eq!(shown, PARAGRAPHS[1]);
    assert_eq!(items[2]["shown_passage"], "p1");
    // The quotation by its span in the sentence and its hash.
    let quotation = &answer(1, "attribution-lookup/1")["detail"]["quotations"][0];
    let reply_sentence = "Varnholm said \"the marigold ferry sails at dawn without fail\".";
    let quoted = &reply_sentence[quotation["quote"]["start"].as_u64().unwrap() as usize
        ..quotation["quote"]["end"].as_u64().unwrap() as usize];
    assert_eq!(quoted, "the marigold ferry sails at dawn without fail");
    assert_eq!(
        quotation["quote"]["sha256"],
        sha256_digest(quoted.as_bytes())
    );
    // Identifiers the release did not sign, by hash.
    assert_eq!(
        items[4]["passage_sha256"],
        sha256_digest("said the crossing was calm".as_bytes())
    );
    assert_eq!(
        items[5]["invalid_cited_sha256"],
        json!([sha256_digest("zephyr-lantern-id".as_bytes())])
    );

    assert_eq!(abstained["detail"]["displayed"], false);
    assert_eq!(abstained["gaps"][0]["reason"], "evidence_missing");
}

fn dossier(path: &Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .arg("inspect")
        .arg(path)
        .output()
        .expect("inspect should start");
    assert!(
        output.status.success(),
        "inspect failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn inspect_prints_both_session_records_with_resolving_line_citations() {
    let home = tempfile::tempdir().unwrap();
    let (path, _, _) = session(home.path());
    let out = dossier(&path);
    for words in RECOGNISABLE {
        assert!(!out.contains(words), "{words:?} is in the dossier:\n{out}");
    }
    let flat = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let at = path.display().to_string();
    assert!(
        flat.contains("integrity-checker v1")
            && flat
                .contains("ruled on 7 reply item(s) under labelled display (labelled requested)")
            && flat.contains("no model stage")
            && flat.contains("fetch 1, quote_shown 2")
            && flat.contains("withheld_attribution 1"),
        "{out}"
    );
    assert!(
        flat.contains("#3 withheld_attribution from_source sentence sha256:"),
        "{out}"
    );
    assert!(
        flat.contains("right_person_quoted by attribution-lookup/1: no (probability 0.0)"),
        "{out}"
    );
    assert!(flat.contains("shows passage p1, bytes"), "{out}");
    assert!(
        flat.contains("#5 quote_shown quote an id the release lacks"),
        "{out}"
    );
    assert!(
        flat.contains("displayed nothing") && flat.contains("failed signature"),
        "{out}"
    );
    assert!(out.contains(&format!("[{at}:1; {at}:1#/payload/detail/counts_vector]")));
    assert!(out.contains(&format!("[{at}:2; {at}:2#/payload/gaps]")));

    // Every citation names a line and, where it has one, a pointer that
    // resolves in the record on that line.
    let records: Vec<Value> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let mut resolved = 0;
    for cite in out
        .split(['[', ']', ';', ' ', '\n'])
        .filter_map(|token| token.strip_prefix(&format!("{at}:")))
    {
        let (line, pointer) = cite.split_once('#').unwrap_or((cite, ""));
        let record = &records[line.parse::<usize>().unwrap() - 1];
        assert!(
            pointer.is_empty() || record.pointer(pointer).is_some(),
            "{cite} does not resolve"
        );
        resolved += 1;
    }
    assert!(resolved >= 10, "{resolved} citations in:\n{out}");
}
