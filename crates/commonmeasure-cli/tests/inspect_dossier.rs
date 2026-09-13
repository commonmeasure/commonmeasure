//! The run dossier, asserted against the committed run artefacts.
//!
//! `inspect`'s claim is that every line it prints resolves to a local record.
//! These tests hold it to that mechanically: run the real binary over the
//! committed example runs, extract every citation the dossier prints, and
//! resolve each one against the artefact it names. A citation that does not
//! resolve is the dossier inventing provenance, which is the defect this
//! command exists to prevent.
//!
//! The negative paths tamper with copies of the committed artefacts and
//! assert the dossier reports the disagreement instead of displaying the
//! tampered value as provenance.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/commonmeasure-cli sits two levels below the repository root")
        .to_path_buf()
}

fn dossier(run: &Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .current_dir(repo_root())
        .args(["inspect"])
        .arg(run)
        .output()
        .expect("inspect should start");
    assert!(
        output.status.success(),
        "inspect failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("the dossier is UTF-8")
}

/// The dossier wraps at a fixed width, so a phrase may span lines. Assertions
/// about wording run against the unwrapped text; citation extraction runs
/// against the raw output, because citations never wrap.
fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn artefact(run: &Path, name: &str) -> Value {
    serde_json::from_slice(&std::fs::read(repo_root().join(run).join(name)).expect(name))
        .expect("valid JSON")
}

/// Every `[...]` group whose content is citation-shaped, split into tokens.
/// Bracketed prose (there is none today) is ignored rather than misparsed.
fn citation_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find(']') else { break };
        let inner = &rest[..end];
        if ["s:", "e:", "r:", "m:"]
            .iter()
            .any(|prefix| inner.starts_with(prefix))
        {
            tokens.extend(inner.split("; ").map(str::to_owned));
        }
        rest = &rest[end..];
    }
    tokens
}

/// The dossier rule: each displayed claim carries an identifier
/// resolving to its local supporting record.
fn assert_every_citation_resolves(run: &Path, text: &str) {
    let summary = artefact(run, "summary.json");
    let manifest = artefact(run, "manifest.json");
    let replay = repo_root()
        .join(run)
        .join("replay.json")
        .exists()
        .then(|| artefact(run, "replay.json"));
    let seqs: Vec<u64> = std::fs::read_to_string(repo_root().join(run).join("evidence.ndjson"))
        .expect("evidence.ndjson")
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|record| record["seq"].as_u64())
        .collect();

    let tokens = citation_tokens(text);
    assert!(
        tokens.len() >= 30,
        "the dossier printed only {} citations; the extractor or the dossier regressed",
        tokens.len()
    );
    for token in &tokens {
        let (prefix, rest) = token.split_at(2);
        let resolves = match prefix {
            "s:" => summary.pointer(rest).is_some(),
            "m:" => manifest.pointer(rest).is_some(),
            "r:" => replay
                .as_ref()
                .is_some_and(|replay| replay.pointer(rest).is_some()),
            "e:" => rest.parse::<u64>().is_ok_and(|seq| seqs.contains(&seq)),
            _ => false,
        };
        assert!(resolves, "citation `{token}` does not resolve in {run:?}");
    }
}

#[test]
fn every_citation_in_the_replay_dossier_resolves_to_a_record() {
    let run = Path::new("demo/output/replay");
    assert_every_citation_resolves(run, &dossier(run));
}

#[test]
fn every_citation_in_the_gateway_dossier_resolves_to_a_record() {
    let run = Path::new("demo/output/tensorzero");
    assert_every_citation_resolves(run, &dossier(run));
}

#[test]
fn every_citation_in_the_skill_dossier_resolves_to_a_record() {
    let run = Path::new("demo/output/skills");
    assert_every_citation_resolves(run, &dossier(run));
}

/// A produced result is traceable input bytes → invocation → result, and the
/// dossier prints the whole chain: what was executed, under whose
/// declaration, with which part of the command line the job supplied.
#[test]
fn an_invocation_is_traceable_from_the_job_value_to_the_result() {
    let run = Path::new("demo/output/skills");
    let text = flat(&dossier(run));
    let summary = artefact(run, "summary.json");
    let invocation = &summary["plans"][1]["acquisition"]["invocation"];

    // The bundle's own declaration, and the digests standing in for the
    // version the format does not have.
    assert!(text.contains("declares itself skill-creator"));
    assert!(
        text.contains("No version: the Agent Skill format has no version key"),
        "the absent version is stated where a reader looks for one"
    );
    assert_eq!(invocation["declared_version"], Value::Null);

    // The job's own value, shown at the position it entered the command line
    // and marked as the job's. This is the input half of the chain.
    assert!(
        text.contains("demo/skills/geo-lookup [job]"),
        "the job's value must be attributed to the job, not to the operator"
    );
    assert!(text.contains("/usr/bin/python3 [operator]"));

    // The result half: a non-zero exit is the verdict, not a failure, and the
    // bytes behind it are sealed under a hash a reader can recompute.
    assert!(text.contains("exited 1 after"));
    assert!(text.contains("produced result — the invocation's standard output"));
    let sealed = std::fs::read(
        repo_root().join(run).join(
            summary["plans"][1]["acquisition"]["response_ref"]
                .as_str()
                .expect("a sealed response"),
        ),
    )
    .expect("the sealed response is committed");
    let recomputed = format!(
        "sha256:{:x}",
        <sha2::Sha256 as sha2::Digest>::digest(&sealed)
    );
    assert!(
        text.contains(&recomputed),
        "the dossier must print the hash the sealed bytes actually have"
    );

    // Two skills, one job, different verdicts — which is the claim the run
    // exists to make, and it is in the selection over both.
    assert!(text.contains("skill:skill-creator-only 0.50"));
    assert!(text.contains("skill:plugin-creator-only 0.17"));
    assert!(
        text.contains("internal-only 0.00"),
        "the operator's own corpus is ranked beside the skills by one objective"
    );
}

/// Which bytes were recorded replay is read from the per-acquisition replay
/// record, and stated with its recording, capture date and hash agreement.
#[test]
fn replayed_bytes_are_declared_as_recordings_with_their_provenance() {
    let text = flat(&dossier(Path::new("demo/output/replay")));

    assert!(text.contains("recorded replay, not live capture"));
    assert!(text.contains("2026-08-01 recording of https://api.exa.ai/search"));
    assert!(text.contains("served from exa/exa-search.json"));
    assert!(
        text.contains("the sealed response hash equals the recorded hash"),
        "the hash agreement the runtime computed must be stated, not implied"
    );
    // The sealed hash is printed whole: it is the value a reader recomputes
    // with sha256sum over responses/exa-only.json.
    assert!(
        text.contains("sha256:9ae9dcd7f39d2082eef5f83747809e23a1fb8128d85fe555616f0b9896753071")
    );
    assert!(text.contains("every sealed response hash equals its recording's"));
    // The evidence log's digest is recomputed from the file, not repeated.
    assert!(text.contains("recomputed here"));

    // Both halves of the recording's provenance are printed verbatim beside
    // the bytes: what was elided from them, and what they may be used for.
    // A reader deciding whether to trust or republish them needs both.
    assert!(
        text.contains("a visible redaction naming $EXA_API_KEY"),
        "the recording's redactions must be stated verbatim"
    );
    assert!(
        text.contains("not licensed content for redistribution or reuse"),
        "the recording's permitted use must be stated verbatim"
    );
}

/// The tensorzero acceptance run acquired from the operator's own corpus at
/// run time. Nothing about it may read as replay.
#[test]
fn a_local_acquisition_is_described_as_capture_not_replay() {
    let text = flat(&dossier(Path::new("demo/output/tensorzero")));

    assert!(!text.contains("recorded replay"));
    assert!(text.contains("local capture — a run-time read of the operator's own supply"));
    assert!(text.contains("live-verified"));
    assert!(
        text.contains("no HTTP status — this was not an HTTP exchange"),
        "a corpus read has no status code and the dossier must say why"
    );
    assert!(text.contains("licence declared (operator-owned/demo-corpus-v1)"));
}

/// Every refusal and absence in the committed replay run, in plain English.
#[test]
fn refusals_and_absences_are_explained_in_plain_english() {
    let text = flat(&dossier(Path::new("demo/output/replay")));

    // TollBit's two refused sources, with the recorded reason.
    assert!(text.contains("refused https://natlawreview.com/article/"));
    assert!(text.contains("The provider returned no excerpt for this result."));
    assert!(text.contains("no content hash — no text was retrieved to hash"));
    // Its inference never ran, and the answer says so rather than going blank.
    assert!(text.contains("not attempted — no inference record exists for this plan"));
    assert!(text.contains("none — no inference completed for this plan"));
    // An undisclosed price is unknown, never zero.
    assert!(text.contains("not disclosed by the provider — an unknown price, never zero"));
    // The gateway field it could not observe.
    assert!(
        text.contains(
            "executed_provider — the gateway's response did not name the upstream provider"
        )
    );
    // The selection abstained and the objective's unmeasured terms are named.
    assert!(text.contains("no plan was selected"));
    assert!(text.contains("Unmeasured objective terms: quality, coverage, freshness, policy_risk"));
    // The suite asked for no citations, so the evaluator measured nothing and
    // the dossier names the switch that would have made it measurable, rather
    // than showing a score no measurement backs.
    assert!(text.contains("the suite did not require a cited answer"));
    assert!(text.contains("set require_cited_answer"));
}

/// One plan's evaluation nulled while the other plans keep theirs: the
/// per-plan rendering branch, which must read the null as absence — "no
/// record" — never as a zero score. This is a partial, tampered state, not a
/// pre-evaluator run; the whole-run state is the test below.
#[test]
fn a_plan_with_a_null_evaluation_renders_absence_not_a_zero_score() {
    let directory = tempfile::tempdir().expect("tempdir");
    let run = directory.path().join("run");
    copy_run(&repo_root().join("demo/output/replay"), &run);

    let mut summary: Value =
        serde_json::from_slice(&std::fs::read(run.join("summary.json")).unwrap()).unwrap();
    summary["plans"][1]["evaluation"] = Value::Null;
    std::fs::write(run.join("summary.json"), summary.to_string()).unwrap();

    let text = flat(&dossier(&run));
    assert!(
        text.contains("this run predates the grounding evaluator"),
        "a missing evaluation must be explained, not rendered as a measurement:\n{text}"
    );
}

/// A mixed-dated window publishes no fraction, and the freshness rule text
/// promises every dated part's own age and verdict still stand in the
/// per-part record — so the dossier must print the rows beneath the
/// unmeasured reason rather than leaving the dated majority readable only in
/// summary.json.
#[test]
fn an_unmeasured_freshness_section_still_lists_its_dated_parts() {
    let directory = tempfile::tempdir().expect("tempdir");
    let run = directory.path().join("run");
    copy_run(&repo_root().join("demo/output/latest"), &run);

    let mut summary: Value =
        serde_json::from_slice(&std::fs::read(run.join("summary.json")).unwrap()).unwrap();
    let plan = summary["plans"]
        .as_array_mut()
        .expect("plans")
        .iter_mut()
        .find(|plan| plan["id"] == "exa-only")
        .expect("the committed selecting run holds exa-only");
    let freshness = &mut plan["evaluation"]["freshness"];
    freshness["unmeasured"] =
        serde_json::json!("1 of 2 admitted parts declares no date, so this window has no fraction");
    freshness["fraction"] = Value::Null;
    freshness["fresh_count"] = Value::Null;
    freshness["parts"][0]["declared_date"] = serde_json::json!("2026-07-31");
    freshness["parts"][0]["age_days"] = serde_json::json!(5);
    freshness["parts"][1] = serde_json::json!({
        "reference": "https://b.example/undated",
        "content_hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        "declared_date": Value::Null,
        "date_provenance": Value::Null,
        "age_days": Value::Null,
        "within_maximum_age": Value::Null,
    });
    std::fs::write(run.join("summary.json"), summary.to_string()).unwrap();

    let raw = dossier(&run);
    let text = flat(&raw);
    assert!(
        text.contains("1 of 2 admitted parts declares no date"),
        "the unmeasured reason must be stated:\n{text}"
    );
    assert!(
        text.contains("declared 2026-07-31, age 5 day(s)"),
        "the dated part's age and verdict still stand beneath the reason:\n{text}"
    );
    assert!(
        text.contains("no declared date; an undated part withholds the window's fraction"),
        "the undated part states itself rather than an unknown age:\n{text}"
    );
    assert_every_citation_resolves(&run, &raw);
}

/// A run published before the grounding evaluator existed: no plan carries an
/// `evaluation` key at all, because the key entered the contract with the
/// evaluator (the manifest side — no sealed `evaluators` — is exercised by
/// the reduced-key fixture in the manifest test). Every plan states the
/// absence, the run-level line says the whole run predates the evaluator,
/// and no citation points at the record that is not there.
#[test]
fn a_run_from_before_the_evaluator_says_so_rather_than_scoring_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let run = directory.path().join("run");
    copy_run(&repo_root().join("demo/output/replay"), &run);

    let mut summary: Value =
        serde_json::from_slice(&std::fs::read(run.join("summary.json")).unwrap()).unwrap();
    let plans = summary["plans"].as_array_mut().expect("plans");
    let plan_count = plans.len();
    assert!(plan_count > 1, "the whole-run state needs several plans");
    for plan in plans.iter_mut() {
        plan.as_object_mut()
            .expect("a plan object")
            .remove("evaluation");
    }
    std::fs::write(run.join("summary.json"), summary.to_string()).unwrap();

    let raw = dossier(&run);
    let text = flat(&raw);
    assert_eq!(
        text.matches("this run predates the grounding evaluator")
            .count(),
        // One per plan, plus the run-level line.
        plan_count + 1,
        "every plan and the run itself must state the absence:\n{text}"
    );
    assert!(
        text.contains("none recorded — no plan carries an evaluation record"),
        "the run-level line must say the whole run predates the evaluator:\n{text}"
    );
    // The absent record is cited by nothing: a pointer to a key the summary
    // does not hold is invented provenance.
    assert!(
        !raw.contains("s:/plans/0/evaluation"),
        "absence has no record to cite:\n{raw}"
    );
    assert_every_citation_resolves(&run, &raw);
}

/// The seal the runtime writes: SHA-256 over the canonical JSON of the
/// `/manifest` object. A test that mutates that object must re-seal with
/// this, or its fixture is internally inconsistent by construction.
fn seal_of(manifest_object: &Value) -> String {
    commonmeasure_types::canonical::canonical_digest(manifest_object)
}

fn copy_run(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("tempdir run");
    for name in [
        "summary.json",
        "manifest.json",
        "evidence.ndjson",
        "replay.json",
    ] {
        std::fs::copy(from.join(name), to.join(name)).expect(name);
    }
    // The sealed response bytes travel with the copy where the source run
    // publishes them, so tempdir tests exercise `inspect` against a run that
    // actually holds the bytes its bindings describe. A run without them is
    // a real state too, and tests for it delete the directory explicitly.
    let responses = from.join("responses");
    if responses.is_dir() {
        let into = to.join("responses");
        std::fs::create_dir_all(&into).expect("responses dir");
        for entry in std::fs::read_dir(&responses).expect("responses dir") {
            let entry = entry.expect("responses entry");
            std::fs::copy(entry.path(), into.join(entry.file_name()))
                .expect("sealed response file");
        }
    }
}

/// The binding's boolean is the runtime's claim, not the verdict: where the
/// sealed bytes are present they are hashed here, and a record that
/// contradicts its own bytes is reported as the disagreement it is, never
/// displayed as provenance.
#[test]
fn a_tampered_replay_binding_is_reported_not_displayed_as_provenance() {
    let directory = tempfile::tempdir().expect("tempdir");
    let run = directory.path().join("run");
    copy_run(&repo_root().join("demo/output/replay"), &run);

    let mut summary: Value =
        serde_json::from_slice(&std::fs::read(run.join("summary.json")).unwrap()).unwrap();
    summary["plans"][1]["acquisition"]["replay"]["matches_recorded_input"] = Value::Bool(false);
    std::fs::write(run.join("summary.json"), summary.to_string()).unwrap();

    let text = flat(&dossier(&run));
    assert!(
        text.contains("RECORD MISMATCH") && text.contains("the record and the bytes disagree"),
        "a record contradicted by its own bytes must be loud:\n{text}"
    );
    // The other plans' bindings are intact and still verify; only the
    // tampered plan's own bytes line must refuse the provenance sentence.
    let exa_bytes_line = text
        .split("recording of https://api.exa.ai/search")
        .nth(1)
        .and_then(|rest| rest.split("redactions").next())
        .expect("the exa plan prints a bytes line");
    assert!(
        !exa_bytes_line.contains("so these are those bytes"),
        "a contradicted binding must not read as verified provenance:\n{exa_bytes_line}"
    );
}

/// One corrupted byte in a sealed response must surface as a hash mismatch:
/// the dossier hashes the bytes on disk rather than restating the boolean
/// the runtime wrote when the bytes were still intact.
#[test]
fn a_corrupted_sealed_response_is_a_mismatch_not_provenance() {
    let directory = tempfile::tempdir().expect("tempdir");
    let run = directory.path().join("run");
    copy_run(&repo_root().join("demo/output/replay"), &run);

    let sealed = run.join("responses/exa-only.json");
    let mut bytes = std::fs::read(&sealed).expect("sealed response");
    bytes[0] ^= 0x01;
    std::fs::write(&sealed, bytes).unwrap();

    let text = flat(&dossier(&run));
    assert!(
        text.contains("MISMATCH: the sealed response does not hash to the recording"),
        "corrupted sealed bytes must be loud:\n{text}"
    );
    assert!(
        text.contains("MISMATCH on exa-only — the sealed bytes do not hash to the recording"),
        "the recordings line must name the corrupted plan:\n{text}"
    );
    assert!(
        !text.contains("every sealed response hash equals its recording's"),
        "a corrupted response must not read as universal agreement:\n{text}"
    );
}

/// A run directory without `responses/` — the committed norm, since sealed
/// response bytes never enter git for most runs — must state that the bytes
/// are absent and that the recorded-hash claim rests on the runtime's own
/// record, rather than printing the producer-vouched sentence as verified.
#[test]
fn a_run_without_sealed_responses_states_absence_not_verification() {
    let directory = tempfile::tempdir().expect("tempdir");
    let run = directory.path().join("run");
    copy_run(&repo_root().join("demo/output/replay"), &run);
    std::fs::remove_dir_all(run.join("responses")).expect("responses dir");

    let text = flat(&dossier(&run));
    assert!(
        text.contains("the sealed response bytes are not present in this directory")
            && text.contains("rests on the runtime's own matches_recorded_input record"),
        "absent bytes must be stated as absence:\n{text}"
    );
    assert!(
        !text.contains("so these are those bytes")
            && !text.contains("every sealed response hash equals its recording's"),
        "an unverifiable claim must not read as verified:\n{text}"
    );
}

/// The dossier states which contract shape it is reading: the current
/// version plainly, an earlier version with a note that it is not current.
#[test]
fn the_contract_line_states_the_schema_version_and_flags_a_stale_one() {
    let current = flat(&dossier(Path::new("demo/output/latest")));
    assert!(
        current.contains("contextops-run/v7 — the current run contract"),
        "a current-contract run must say so:\n{current}"
    );

    // The committed replay example still declares the v3 contract it was
    // published under; it is read as it declares itself and flagged as not
    // current, never rendered indistinguishably from a current run.
    let stale = flat(&dossier(Path::new("demo/output/replay")));
    assert!(
        stale.contains("contextops-run/v3 — NOT the current contract (contextops-run/v7)"),
        "an earlier-contract run must be flagged, not rendered indistinguishably:\n{stale}"
    );
}

/// An evaluated plan's grounding section prints the evaluator's identity,
/// the distinct verdict counts and one line per citation — supported with
/// its byte span, everything else with its reason.
#[test]
fn an_evaluated_plan_renders_its_verdicts_distinctly_with_spans() {
    let directory = tempfile::tempdir().expect("tempdir");
    let run = directory.path().join("run");
    copy_run(&repo_root().join("demo/output/replay"), &run);

    let mut summary: Value =
        serde_json::from_slice(&std::fs::read(run.join("summary.json")).unwrap()).unwrap();
    summary["plans"][1]["evaluation"] = serde_json::json!({
        "record_version": "contextops-evaluation/v1",
        "evaluator": {
            "name": "grounding",
            "version": "0.1.0",
            "configuration_digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
        },
        "method": "test rendering",
        "inputs": [],
        "verdict_counts": {"supported": 1, "contradicted": 0, "uncovered": 1, "unavailable": 0},
        "citations": [
            {
                "url": "https://example.org/a",
                "quote": "the exact words",
                "verdict": "supported",
                "matched": {"content_hash": "sha256:2222222222222222222222222222222222222222222222222222222222222222", "start": 10, "end": 25},
                "reason": "the quote occurs verbatim in the cited source's retained text",
            },
            {
                "url": "https://elsewhere.example",
                "quote": "other words",
                "verdict": "uncovered",
                "matched": null,
                "reason": "the cited URL is not among the sources that entered this plan's window",
            },
        ],
        "unevaluated": null,
        "assurance": "observed",
        "blind_spots": [],
    });
    std::fs::write(run.join("summary.json"), summary.to_string()).unwrap();

    let text = flat(&dossier(&run));
    assert!(
        text.contains("judged 2 citation(s)")
            && text.contains("1 supported, 0 contradicted, 1 uncovered, 0 unavailable"),
        "the verdict counts must stay distinct:\n{text}"
    );
    assert!(
        text.contains("occurs at bytes 10-25 of the retained text"),
        "a supported citation must print its span:\n{text}"
    );
    // The fixture's content hash is fabricated, so nothing in the sealed
    // response hashes to it: the dossier must say the span rests on the
    // record rather than presenting it as rechecked — or as a mismatch.
    assert!(
        text.contains("hashes to the record's content hash")
            && text.contains("rests on the record's own claim and is not rechecked here"),
        "an unlocatable retained text must be stated as unrechecked:\n{text}"
    );
    assert!(
        !text.contains("SPAN MISMATCH"),
        "unlocatable is not a contradiction:\n{text}"
    );
    assert!(
        text.contains("#2 uncovered") && text.contains("https://elsewhere.example"),
        "an uncovered citation must print its URL and reason:\n{text}"
    );
}

/// The sealed responses of the cited run, or a stated reason the tests that
/// need them cannot run here: `responses/` is gitignored — sealed provider
/// bytes never commit — so on a fresh clone there is nothing to excise a
/// span from, and pretending to have checked would be this suite committing
/// the sin it polices.
fn cited_sealed_responses() -> Option<PathBuf> {
    let responses = repo_root().join("demo/output/cited/responses");
    if responses.is_dir() {
        return Some(responses);
    }
    eprintln!(
        "skipping: demo/output/cited/responses is not present in this checkout — sealed \
         response bytes are gitignored and never commit — so the retained text cannot be \
         excised here; the committed half of this check still runs in \
         commonmeasure-runtime/tests/cited_artefact.rs, and `just cited-example` regenerates the bytes"
    );
    None
}

/// The published cited run's supported spans, rechecked by the dossier
/// against the sealed bytes: the retained text is located in the sealed
/// response by its content hash and the span excises to the quoted words.
/// The parts the transform stage reshaped are stated as resting on the
/// invocation record that carries their hash, never as rechecked.
#[test]
fn a_published_supported_span_is_rechecked_against_the_sealed_response() {
    if cited_sealed_responses().is_none() {
        return;
    }
    let run = Path::new("demo/output/cited");
    let text = flat(&dossier(run));
    assert!(
        text.contains("the span excises to the quoted words, rechecked here"),
        "at least one published span must be rechecked against the sealed bytes:\n{text}"
    );
    assert!(
        text.contains("the retained text is the transformed window text"),
        "a transformed part must state its tie to the invocation record:\n{text}"
    );
    assert!(
        !text.contains("SPAN MISMATCH"),
        "the committed spans must recheck cleanly:\n{text}"
    );
}

/// A recorded span shifted under an untouched quote must surface as a span
/// mismatch: the dossier excises the sealed bytes rather than restating the
/// record's own claim about them.
#[test]
fn a_tampered_supported_span_is_a_mismatch_not_provenance() {
    if cited_sealed_responses().is_none() {
        return;
    }
    let directory = tempfile::tempdir().expect("tempdir");
    let run = directory.path().join("run");
    copy_run(&repo_root().join("demo/output/cited"), &run);

    let mut summary: Value =
        serde_json::from_slice(&std::fs::read(run.join("summary.json")).unwrap()).unwrap();
    let matched = &mut summary["plans"][1]["evaluation"]["grounding"]["citations"][0]["matched"];
    assert_eq!(
        matched["content_hash"],
        "sha256:d32e90d30bb4e42670c8d811a118f99c47118543d0e8f2bd869e57718b302b4c",
        "the tampered citation must be one whose retained text the sealed response carries"
    );
    let start = matched["start"].as_u64().expect("a span start");
    matched["start"] = serde_json::json!(start + 2);
    std::fs::write(run.join("summary.json"), summary.to_string()).unwrap();

    let text = flat(&dossier(&run));
    assert!(
        text.contains("SPAN MISMATCH")
            && text.contains("the recorded span does not excise to this quote"),
        "a span the bytes do not back must be loud:\n{text}"
    );
}

/// The cited run's fidelity lines: the verifier's per-claim verdicts, each
/// rechecked against the answer's own bytes, and the judge's agreement
/// figure beside them.
#[test]
fn the_cited_dossier_carries_the_fidelity_verdicts_and_the_judge_agreement() {
    let text = flat(&dossier(Path::new("demo/output/cited")));
    assert!(
        text.contains("fidelity-verifier v1")
            && text.contains("the claim excises from the answer at bytes"),
        "the verifier's claims must be printed and rechecked:\n{text}"
    );
    assert!(
        text.contains("fidelity-judge v1") && text.contains("agreement fraction"),
        "the judge's agreement figure must be printed beside the verdicts:\n{text}"
    );
    assert!(
        !text.contains("CLAIM MISMATCH"),
        "the committed claim spans must recheck cleanly:\n{text}"
    );
}

/// A claim span shifted under an untouched claim text is a mismatch: the
/// dossier excises the answer rather than restating the record's claim.
#[test]
fn a_tampered_claim_span_is_a_mismatch_not_provenance() {
    let directory = tempfile::tempdir().expect("tempdir");
    let run = directory.path().join("run");
    copy_run(&repo_root().join("demo/output/cited"), &run);

    let mut summary: Value =
        serde_json::from_slice(&std::fs::read(run.join("summary.json")).unwrap()).unwrap();
    let plan = &mut summary["plans"][1];
    let verifier = plan["processors"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|invocation| invocation["processor"]["name"] == "fidelity-verifier")
        .expect("the exa plan carries the verifier");
    let claim = &mut verifier["detail"]["claims"][0];
    let start = claim["start"].as_u64().expect("a claim start");
    claim["start"] = serde_json::json!(start + 1);
    std::fs::write(run.join("summary.json"), summary.to_string()).unwrap();

    let text = flat(&dossier(&run));
    assert!(
        text.contains("CLAIM MISMATCH"),
        "a claim span the answer does not back must be loud:\n{text}"
    );
}

/// Without the sealed bytes — the committed norm — the span is stated
/// against the content hash and the dossier names where the retained text
/// can be obtained, rather than printing a byte range into text no artefact
/// in the directory holds.
#[test]
fn an_unpublished_retained_text_is_named_as_obtainable_not_left_unresolvable() {
    let directory = tempfile::tempdir().expect("tempdir");
    let run = directory.path().join("run");
    copy_run(&repo_root().join("demo/output/cited"), &run);
    let responses = run.join("responses");
    if responses.is_dir() {
        std::fs::remove_dir_all(&responses).expect("responses dir");
    }

    let text = flat(&dossier(&run));
    assert!(
        text.contains("the sealed response bytes are not present in this directory")
            && text.contains("the span is stated against the content hash")
            && text.contains("regenerating the run, which republishes responses/"),
        "the reader must be told where the retained text lives:\n{text}"
    );
    assert!(
        !text.contains("excises to the quoted words"),
        "nothing may read as excised where there is nothing to excise from:\n{text}"
    );
}

/// The tally the grounding line prints is counted from the citation records;
/// a `verdict_counts` claim that disagrees with them is reported as a
/// mismatch, never displayed as the measurement. This is the disagreeing
/// branch the agreeing fixture above cannot reach.
#[test]
fn a_mutated_verdict_count_is_a_count_mismatch_not_the_tally() {
    let directory = tempfile::tempdir().expect("tempdir");
    let run = directory.path().join("run");
    copy_run(&repo_root().join("demo/output/replay"), &run);

    let mut summary: Value =
        serde_json::from_slice(&std::fs::read(run.join("summary.json")).unwrap()).unwrap();
    summary["plans"][1]["evaluation"] = serde_json::json!({
        "record_version": "contextops-evaluation/v1",
        "evaluator": {
            "name": "grounding",
            "version": "0.1.0",
            "configuration_digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
        },
        "method": "test rendering",
        "inputs": [],
        "verdict_counts": {"supported": 9, "contradicted": 0, "uncovered": 1, "unavailable": 0},
        "citations": [
            {
                "url": "https://elsewhere.example",
                "quote": "other words",
                "verdict": "uncovered",
                "matched": null,
                "reason": "the cited URL is not among the sources that entered this plan's window",
            },
        ],
        "unevaluated": null,
        "assurance": "observed",
        "blind_spots": [],
    });
    std::fs::write(run.join("summary.json"), summary.to_string()).unwrap();

    let text = flat(&dossier(&run));
    assert!(
        text.contains("judged 1 citation(s)")
            && text.contains("0 supported, 0 contradicted, 1 uncovered, 0 unavailable"),
        "the printed tally must be counted from the citations:\n{text}"
    );
    assert!(
        text.contains("COUNT MISMATCH: verdict_counts claims 9 supported"),
        "a disagreeing verdict_counts must be loud:\n{text}"
    );
    assert!(
        !text.contains("window: 9 supported"),
        "the claimed counts must not be printed as the tally:\n{text}"
    );
}

/// The sealed inputs the `manifest` line names, in the order it prints them.
fn named_sealed_inputs(text: &str) -> Vec<String> {
    let flat = flat(text);
    let marker = "seals every key of manifest.json's /manifest object: ";
    let start = flat
        .find(marker)
        .unwrap_or_else(|| panic!("the manifest line must name what it seals:\n{flat}"))
        + marker.len();
    let rest = &flat[start..];
    let end = rest
        .find(';')
        .expect("the inventory ends where the corroboration clause begins");
    rest[..end].split(", ").map(str::to_owned).collect()
}

/// The `manifest` line's inventory is the manifest's own key set, because the
/// hash is taken over that object whole. A literal list here would drift the
/// next time an input is sealed, and did: the dossier is where a reader asks
/// which inputs are part of experiment identity.
#[test]
fn the_manifest_line_names_exactly_what_the_manifest_seals() {
    // The fixture set is exactly the committed runs. `demo/output/live` is
    // deliberately absent: it is the local live-run convention, gitignored,
    // so a loop consuming it passes or fails as a property of one machine's
    // filesystem rather than of the repository. It is also the repository's
    // only contextops-manifest/v1 artefact; the reduced-key fixture below
    // keeps the earlier-inventory rendering exercised in its place.
    for (run, recipe) in [
        ("demo/output/replay", "just replay-example"),
        ("demo/output/latest", "just rubric-example"),
        ("demo/output/cited", "just cited-example"),
        ("demo/output/tensorzero", "just internal-corpus-example"),
    ] {
        let run = Path::new(run);
        assert!(
            repo_root().join(run).join("manifest.json").exists(),
            "{run:?} is a committed artefact this test drives; restore it from git or \
             regenerate it with `{recipe}`"
        );
        let text = dossier(run);
        let mut named = named_sealed_inputs(&text);
        let mut sealed: Vec<String> = artefact(run, "manifest.json")["manifest"]
            .as_object()
            .expect("manifest.json holds the sealed object at /manifest")
            .keys()
            .cloned()
            .collect();
        named.sort();
        sealed.sort();
        assert_eq!(
            named, sealed,
            "the manifest line for {run:?} does not name the set the hash seals"
        );
        // Every committed seal re-derives from its own /manifest object: the
        // recomputation the dossier performs must corroborate, not mismatch.
        assert!(
            flat(&text).contains("the /manifest object hashes to that seal, recomputed here"),
            "the committed seal for {run:?} did not re-derive from its /manifest object"
        );
    }

    // The v3 bump exists to seal the suite's declared coverage rubric and
    // as-of reference beside the evaluator identities, so a suite that
    // declares either is a different experiment by hash.
    let current = named_sealed_inputs(&dossier(Path::new("demo/output/latest")));
    assert!(
        current.iter().any(|key| key == "coverage_rubric")
            && current.iter().any(|key| key == "as_of")
            && current.iter().any(|key| key == "evaluators"),
        "a v3 artefact must name the fields the manifest was bumped for: {current:?}"
    );
    // And an artefact sealing a different set describes itself, rather than
    // borrowing the inventory of the manifest version it is read beside. The
    // earlier key set is supplied here — and
    // re-sealed, because the dossier recomputes the seal from the object and
    // a fixture carrying the old hash over the reduced set would rightly be
    // reported as tampered.
    let directory = tempfile::tempdir().expect("tempdir");
    let run = directory.path().join("run");
    copy_run(&repo_root().join("demo/output/replay"), &run);
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(run.join("manifest.json")).unwrap()).unwrap();
    manifest["manifest"]
        .as_object_mut()
        .expect("manifest.json holds the sealed object at /manifest")
        .remove("evaluators");
    let reseal = seal_of(&manifest["manifest"]);
    manifest["hash"] = Value::String(reseal.clone());
    std::fs::write(run.join("manifest.json"), manifest.to_string()).unwrap();
    let mut summary: Value =
        serde_json::from_slice(&std::fs::read(run.join("summary.json")).unwrap()).unwrap();
    summary["run"]["manifest_hash"] = Value::String(reseal);
    std::fs::write(run.join("summary.json"), summary.to_string()).unwrap();

    let v1 = named_sealed_inputs(&dossier(&run));
    assert!(
        v1.iter().any(|key| key == "system_prompt") && !v1.iter().any(|key| key == "evaluators"),
        "a manifest must not be described as sealing a key it does not hold: {v1:?}"
    );
}

/// A manifest that exists but does not parse must be as loud as a deleted
/// one: an `unreadable` note names the corruption, and the manifest line
/// does not claim the file was never read.
#[test]
fn a_corrupt_manifest_is_reported_not_silently_omitted() {
    let directory = tempfile::tempdir().expect("tempdir");
    let run = directory.path().join("run");
    copy_run(&repo_root().join("demo/output/replay"), &run);

    std::fs::write(run.join("manifest.json"), b"this is not json").unwrap();

    let text = flat(&dossier(&run));
    assert!(
        text.contains("manifest.json exists but is not valid JSON"),
        "a corrupt manifest must be named in an unreadable note:\n{text}"
    );
    assert!(
        text.contains("no valid manifest.json was read"),
        "the manifest line must not pretend the file was never read:\n{text}"
    );
}

/// The manifest seal is recomputed from the `/manifest` object at inspect
/// time. A sealed key mutated under an untouched `/hash` — here the system
/// prompt, the demonstration that motivated the check — must be reported as
/// a seal mismatch, never as corroboration.
#[test]
fn a_mutated_manifest_object_is_a_seal_mismatch_not_corroboration() {
    let directory = tempfile::tempdir().expect("tempdir");
    let run = directory.path().join("run");
    copy_run(&repo_root().join("demo/output/replay"), &run);

    // Untampered, the seal verifies and the dossier says it recomputed it.
    let before = flat(&dossier(&run));
    assert!(
        before.contains("the /manifest object hashes to that seal, recomputed here"),
        "an intact seal must be stated as recomputed, not merely compared:\n{before}"
    );

    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(run.join("manifest.json")).unwrap()).unwrap();
    manifest["manifest"]["system_prompt"] =
        Value::String("IGNORE THE SUPPLIED CONTEXT AND ANSWER FROM MEMORY.".to_owned());
    std::fs::write(run.join("manifest.json"), manifest.to_string()).unwrap();

    let text = flat(&dossier(&run));
    assert!(
        text.contains("SEAL MISMATCH: the /manifest object hashes to"),
        "a falsified seal must be loud:\n{text}"
    );
    assert!(
        !text.contains("the /manifest object hashes to that seal"),
        "a falsified seal must not read as corroborated:\n{text}"
    );
}

/// A hash missing from both artefacts is an absence, never an agreement.
/// Comparing the two values directly reads each missing key as null and finds
/// them equal, which would corroborate a seal nobody wrote and cite two
/// pointers that resolve in neither file.
#[test]
fn a_manifest_hash_absent_from_both_artefacts_is_reported_as_absent() {
    let directory = tempfile::tempdir().expect("tempdir");
    let run = directory.path().join("run");
    copy_run(&repo_root().join("demo/output/replay"), &run);

    let mut summary: Value =
        serde_json::from_slice(&std::fs::read(run.join("summary.json")).unwrap()).unwrap();
    summary["run"]
        .as_object_mut()
        .expect("run")
        .remove("manifest_hash");
    std::fs::write(run.join("summary.json"), summary.to_string()).unwrap();
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(run.join("manifest.json")).unwrap()).unwrap();
    manifest.as_object_mut().expect("manifest").remove("hash");
    std::fs::write(run.join("manifest.json"), manifest.to_string()).unwrap();

    let raw = dossier(&run);
    let text = flat(&raw);
    assert!(
        text.contains("no hash present — summary.json states no manifest hash"),
        "a summary without a manifest hash must say so:\n{text}"
    );
    assert!(
        text.contains("manifest.json has no hash present at /hash"),
        "a manifest without a hash must say so:\n{text}"
    );
    assert!(
        !text.contains("carries the identical hash"),
        "two absences are not an agreement:\n{text}"
    );
    assert_every_citation_resolves(&run, &raw);
}

/// The token basis is glossed from the recorded value. A run counting tokens
/// some other way must not be described as counting words.
#[test]
fn an_unrecognised_token_basis_is_named_not_described_as_word_counting() {
    let directory = tempfile::tempdir().expect("tempdir");
    let run = directory.path().join("run");
    copy_run(&repo_root().join("demo/output/replay"), &run);

    let mut summary: Value =
        serde_json::from_slice(&std::fs::read(run.join("summary.json")).unwrap()).unwrap();
    summary["run"]["token_basis"] = Value::String("model-tokeniser".to_owned());
    std::fs::write(run.join("summary.json"), summary.to_string()).unwrap();

    let text = flat(&dossier(&run));
    assert!(
        text.contains(
            "model-tokeniser — every token count below is counted on the \
                       model-tokeniser basis; read docs/contracts/run-output.md"
        ),
        "an unrecognised basis is named and pointed at the contract:\n{text}"
    );
    assert!(
        !text.contains("not a model tokenisation"),
        "the gloss for whitespace-words must not be printed over another basis:\n{text}"
    );
}

/// An evidence record without a seq cannot be cited. `e:0` names the line
/// before the first record the runtime writes, so it is provenance no log
/// holds.
#[test]
fn an_evidence_record_without_a_seq_is_cited_by_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let run = directory.path().join("run");
    copy_run(&repo_root().join("demo/output/replay"), &run);

    let stripped: String = std::fs::read_to_string(run.join("evidence.ndjson"))
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .map(|mut record| {
            record.as_object_mut().expect("record").remove("seq");
            format!("{record}\n")
        })
        .collect();
    std::fs::write(run.join("evidence.ndjson"), stripped).unwrap();

    let raw = dossier(&run);
    assert!(
        !raw.contains("e:0"),
        "a record with no seq must be cited by nothing, not by zero:\n{raw}"
    );
    assert_every_citation_resolves(&run, &raw);
}

/// A replay.json that exists but does not parse must be reported, not
/// silently dropped along with the run's recordings verification line.
#[test]
fn a_corrupt_replay_record_is_reported_not_silently_omitted() {
    let directory = tempfile::tempdir().expect("tempdir");
    let run = directory.path().join("run");
    copy_run(&repo_root().join("demo/output/replay"), &run);

    std::fs::write(run.join("replay.json"), b"{broken").unwrap();

    let text = flat(&dossier(&run));
    assert!(
        text.contains("replay.json exists but is not valid JSON"),
        "a corrupt replay record must be named in an unreadable note:\n{text}"
    );
}

/// A charge whose two halves agree is printed once; halves that disagree are
/// both printed, because folding one away would hide the disagreement.
#[test]
fn disagreeing_charge_halves_are_both_displayed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let run = directory.path().join("run");
    copy_run(&repo_root().join("demo/output/replay"), &run);

    let mut summary: Value =
        serde_json::from_slice(&std::fs::read(run.join("summary.json")).unwrap()).unwrap();
    // The committed exa charge is 0.007 USD in both halves and displays once.
    assert!(flat(&dossier(&run)).contains("charge 0.007 USD (observed"));
    summary["plans"][1]["acquisition"]["charge"]["native"]["amount"] = serde_json::json!(0.009);
    std::fs::write(run.join("summary.json"), summary.to_string()).unwrap();

    assert!(
        flat(&dossier(&run)).contains("charge 0.007 USD; 0.009 USD (observed"),
        "a native half that no longer restates the money half must be shown beside it"
    );
}

/// Admit-stage invocations are reported per processor identity: a second
/// screening processor gets its own line and its own counts, never absorbed
/// into the first processor's.
#[test]
fn admit_invocations_are_grouped_by_processor_identity() {
    let directory = tempfile::tempdir().expect("tempdir");
    let run = directory.path().join("run");
    copy_run(&repo_root().join("demo/output/replay"), &run);

    let mut summary: Value =
        serde_json::from_slice(&std::fs::read(run.join("summary.json")).unwrap()).unwrap();
    // The exa plan carries two pii-detector admit invocations. Install a
    // second, differently named admit processor beside them.
    let second = serde_json::json!({
        "processor": {
            "name": "second-screen",
            "version": "0.0.1",
            "configuration_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        },
        "stage": "admit",
        "decision": "admit",
        "detail": {"findings": []},
        "method": "a second admission judgement, for this test only",
        "blind_spots": ["none declared"],
    });
    summary["plans"][1]["processors"]
        .as_array_mut()
        .expect("processors")
        .push(second);
    std::fs::write(run.join("summary.json"), summary.to_string()).unwrap();

    let text = flat(&dossier(&run));
    assert!(
        text.contains("pii-detector v") && text.contains("judged 2 source text(s)"),
        "the first processor keeps its own line and count:\n{text}"
    );
    assert!(
        text.contains("second-screen v0.0.1") && text.contains("judged 1 source text(s)"),
        "a second admit processor gets its own screening line:\n{text}"
    );
}

/// An evidence log that is not the one the summary attests to must be
/// reported: the digest is recomputed from the file on every inspect.
#[test]
fn a_grown_evidence_log_is_reported_as_a_digest_mismatch() {
    let directory = tempfile::tempdir().expect("tempdir");
    let run = directory.path().join("run");
    copy_run(&repo_root().join("demo/output/replay"), &run);

    let mut log = std::fs::read(run.join("evidence.ndjson")).unwrap();
    log.extend_from_slice(
        b"{\"seq\":17,\"timestamp\":\"2026-08-03T09:00:00Z\",\"event\":\"run_completed\",\"payload\":{}}\n",
    );
    std::fs::write(run.join("evidence.ndjson"), log).unwrap();

    let text = flat(&dossier(&run));
    assert!(
        text.contains("DIGEST MISMATCH"),
        "a log the summary does not attest to must be loud:\n{text}"
    );
}

/// The committed licensed replay run's dossier shows the whole quote-then-buy
/// trail in the order it happened: the price and billing note before the
/// purchase decision, both sealed quote legs with their hashes, and the
/// receipt recorded as trial coverage rather than free money.
#[test]
fn the_licensed_replay_dossier_shows_the_quote_before_the_purchase_decision() {
    let text = flat(&dossier(Path::new("demo/output/redpine-replay")));
    let quote = text
        .find("priced before purchase")
        .expect("the quote line exists");
    let decision = text
        .find("purchase decision confirmed")
        .expect("the decision line exists");
    assert!(
        quote < decision,
        "the quote is shown before the decision that rested on it"
    );
    assert!(
        text.contains("billing note: Covered by free trial"),
        "the provider's own billing sentence is shown verbatim:\n{text}"
    );
    assert!(
        text.contains("responses/redpine-only-inspect.json")
            && text.contains("responses/redpine-only-quote.json"),
        "both sealed quote legs are named:\n{text}"
    );
    assert!(
        text.contains("1 trial_queries") && text.contains("meter, not a price"),
        "the receipt reads as trial coverage, never free:\n{text}"
    );
    assert!(
        text.contains("recorded replay, not live capture")
            && text.contains("redpine/redpine-confirm-sample.json"),
        "the settlement bytes read as replay, bound to their recording:\n{text}"
    );
}

/// No summary, no dossier: the failure names the file rather than printing a
/// partial document.
#[test]
fn a_directory_without_a_summary_is_an_error_naming_the_file() {
    let directory = tempfile::tempdir().expect("tempdir");
    let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["inspect"])
        .arg(directory.path())
        .output()
        .expect("inspect should start");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("summary.json"));
}
