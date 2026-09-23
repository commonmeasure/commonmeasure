//! The published cited run, witnessed where it stands: `output/cited`
//! is the first artefact this project has ever published with `supported`
//! verdicts, and until it, the supported rendering path had never met a real
//! run. These tests hold the published record to the check the evaluator
//! performs in memory (`find_quote`, via `grounding`), performed once against
//! the committed artefact.
//!
//! Two halves, because the artefact publishes in two halves. The committed
//! `summary.json` carries the structural facts — spans, content hashes, the
//! record's own `inputs` — and the first test asserts them from committed
//! files alone. The retained window text is deliberately not committed: plan
//! sources pin it by content hash only, and `responses/` is gitignored
//! because sealed provider bytes may carry licensed content. So the excision
//! half runs only where the sealed bytes are present, locating each retained
//! text inside them by its hash — received across the boundary, never
//! rebuilt — and skips with its reason stated where they are not.

use commonmeasure_inference::ContextPart;
use commonmeasure_runtime::evaluate::{EvaluationInput, grounding};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn run_dir() -> PathBuf {
    Path::new(env!("COMMONMEASURE_EVIDENCE_DIR")).join("output/cited")
}

fn summary() -> Value {
    let path = run_dir().join("summary.json");
    serde_json::from_slice(&std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "{} is a committed artefact this test drives ({error}); restore it from git or \
             regenerate it with `just cited-example`",
            path.display()
        )
    }))
    .expect("summary.json is valid JSON")
}

/// The plan's grounding record. The run is published under the current
/// contract, which nests one section per evaluator.
fn grounding_record(plan: &Value) -> &Value {
    &plan["evaluation"]["grounding"]
}

/// Every supported citation a plan's record publishes.
fn supported_citations(plan: &Value) -> Vec<Value> {
    grounding_record(plan)["citations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|citation| citation["verdict"] == "supported")
        .cloned()
        .collect()
}

/// The structural half, from committed files alone: the published run
/// carries at least one `supported` citation, and every one pins a byte span
/// and a content hash that the record's own `inputs` list as a window part
/// under the cited URL. A `supported` verdict whose span pointed at a hash
/// the window never held would be provenance into nothing.
#[test]
#[cfg_attr(
    not(evidence_output),
    ignore = "needs the committed runs: output/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn the_published_run_carries_supported_verdicts_pinned_to_window_hashes() {
    let summary = summary();
    let mut supported = 0usize;
    for plan in summary["plans"].as_array().into_iter().flatten() {
        let inputs = grounding_record(plan)["inputs"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for citation in supported_citations(plan) {
            supported += 1;
            let matched = &citation["matched"];
            let (start, end) = (
                matched["start"].as_u64().expect("a recorded span start"),
                matched["end"].as_u64().expect("a recorded span end"),
            );
            assert!(
                start < end,
                "plan {} publishes an empty or inverted span {start}-{end}",
                plan["id"]
            );
            let hash = matched["content_hash"]
                .as_str()
                .expect("a recorded content hash");
            assert!(hash.starts_with("sha256:"), "an unpinned hash: {hash}");
            assert!(
                inputs.iter().any(|input| {
                    input["content_hash"] == *hash && input["reference"] == citation["url"]
                }),
                "plan {}: the span's content hash {hash} must name a window part the \
                 record's own inputs list under {}",
                plan["id"],
                citation["url"]
            );
        }
    }
    assert!(
        supported >= 1,
        "the published cited run must carry at least one supported citation; if a \
         regeneration produced none, the supported path has lost its only real witness"
    );
}

/// The first string anywhere in `value` whose SHA-256 is `hash`. The hash is
/// the receiver: whatever the response's shape, a string that hashes to the
/// record's pin *is* the retained text the record measured, and a test that
/// re-parsed the response the way it guesses the adapter does would be
/// rebuilding the bug's hiding place instead.
fn string_hashing_to(value: &Value, hash: &str) -> Option<String> {
    match value {
        Value::String(text) => {
            (format!("sha256:{:x}", Sha256::digest(text.as_bytes())) == hash).then(|| text.clone())
        }
        Value::Array(items) => items.iter().find_map(|item| string_hashing_to(item, hash)),
        Value::Object(map) => map.values().find_map(|item| string_hashing_to(item, hash)),
        _ => None,
    }
}

/// The excision half: each published span, rechecked by the production
/// evaluator against retained text located in the sealed response bytes by
/// its content hash. The recomputed `matched` object must equal the
/// published one byte for byte — same hash, same start, same end — which is
/// `evaluate.rs`'s in-memory check performed against the published record.
///
/// Window parts the transform stage reshaped are not carried verbatim in any
/// sealed response, so their spans cannot be located here; they are covered
/// by the structural half above, and at least one span per run must locate
/// and recheck or this test fails rather than passes by vacancy.
#[test]
#[cfg_attr(
    not(evidence_output),
    ignore = "needs the committed runs: output/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn a_published_supported_span_excises_from_the_retained_text_to_its_quote() {
    let summary = summary();
    if !run_dir().join("responses").is_dir() {
        eprintln!(
            "skipping the excision half: output/cited/responses is not present in this \
             checkout — sealed response bytes are gitignored and never commit, and the \
             retained text exists nowhere else — so there is nothing to excise from. The \
             structural half above still ran from committed files; `just cited-example` \
             regenerates the sealed bytes"
        );
        return;
    }

    let mut rechecked = 0usize;
    for plan in summary["plans"].as_array().into_iter().flatten() {
        let published = supported_citations(plan);
        if published.is_empty() {
            continue;
        }
        let response: Value = serde_json::from_slice(
            &std::fs::read(
                run_dir().join(
                    plan["acquisition"]["response_ref"]
                        .as_str()
                        .expect("an evaluated plan seals its response"),
                ),
            )
            .expect("the sealed response bytes"),
        )
        .expect("the sealed response is valid JSON");

        // The window parts, received rather than rebuilt: each is text the
        // sealed response carries that hashes to the pin the record's own
        // inputs put on it, under the reference those inputs name.
        let parts: Vec<ContextPart> = grounding_record(plan)["inputs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|input| input["reference"] != "answer")
            .filter_map(|input| {
                let hash = input["content_hash"].as_str()?;
                Some(ContextPart {
                    source_ref: input["reference"].as_str()?.to_owned(),
                    content_hash: hash.to_owned(),
                    text: string_hashing_to(&response, hash)?,
                })
            })
            .collect();
        if parts.is_empty() {
            continue;
        }

        let record = grounding(&EvaluationInput {
            answer: plan["answer"].as_str(),
            cited_answer_required: true,
            parts: &parts,
        });
        for citation in &published {
            let Some(part) = parts
                .iter()
                .find(|part| part.content_hash == citation["matched"]["content_hash"])
            else {
                // A transformed part: its retained text is not verbatim in
                // the sealed response, so this citation has nothing here to
                // recheck against.
                continue;
            };
            let recomputed = record["citations"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|recomputed| recomputed["url"] == citation["url"])
                .unwrap_or_else(|| {
                    panic!(
                        "the recomputed record judges the citation of {}",
                        part.source_ref
                    )
                });
            assert_eq!(
                recomputed["verdict"], "supported",
                "the published answer's citation of {} no longer verifies against the \
                 sealed retained text",
                part.source_ref
            );
            assert_eq!(
                recomputed["matched"], citation["matched"],
                "the published span for {} is not the span the evaluator recomputes from \
                 the sealed retained text",
                part.source_ref
            );
            rechecked += 1;
        }
    }
    assert!(
        rechecked >= 1,
        "no published supported span could be located in the sealed responses and \
         rechecked; the excision check passed by vacancy, which is not a pass"
    );
}

/// The invocation of one named processor on a plan, if it ran.
fn invocation<'a>(plan: &'a Value, name: &str) -> Option<&'a Value> {
    plan["processors"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|invocation| invocation["processor"]["name"] == name)
}

/// The fidelity acceptance evidence, pinned where it stands: the published
/// run carries a supported verdict whose span starts inside the retained
/// text rather than at its opening words, every answered plan with a window
/// carries the verifier's per-claim verdicts, and the judge's agreement
/// figure is published beside them with at least one claim compared.
#[test]
#[cfg_attr(
    not(evidence_output),
    ignore = "needs the committed runs: output/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn the_published_run_carries_a_mid_text_supported_span_and_a_judge_agreement_figure() {
    let summary = summary();
    let mut mid_text = 0usize;
    let mut judged = 0usize;
    for plan in summary["plans"].as_array().into_iter().flatten() {
        mid_text += supported_citations(plan)
            .iter()
            .filter(|citation| citation["matched"]["start"].as_u64().unwrap_or_default() > 0)
            .count();
        if plan["status"] != "completed" || plan["source_count"].as_u64() == Some(0) {
            continue;
        }
        let verifier = invocation(plan, "fidelity-verifier")
            .unwrap_or_else(|| panic!("plan {} carries no verifier invocation", plan["id"]));
        assert_eq!(verifier["stage"], "verify");
        assert!(
            verifier["detail"]["claim_count"]
                .as_u64()
                .unwrap_or_default()
                >= 1,
            "plan {} was verified against no claims",
            plan["id"]
        );
        for claim in verifier["detail"]["claims"]
            .as_array()
            .into_iter()
            .flatten()
        {
            assert!(
                ["supported", "contradicted", "unsupported"]
                    .contains(&claim["verdict"].as_str().unwrap_or_default()),
                "a claim outside the verdict vocabulary: {claim}"
            );
        }
        let judge = invocation(plan, "fidelity-judge")
            .unwrap_or_else(|| panic!("plan {} carries no judge invocation", plan["id"]));
        assert_eq!(judge["assurance"], "declared");
        assert_eq!(judge["detail"]["judged"], true, "{}", plan["id"]);
        let agreement = &judge["detail"]["agreement"];
        assert!(agreement["compared"].as_u64().unwrap_or_default() >= 1);
        assert!(
            agreement["fraction"].is_number(),
            "the agreement figure must be published where claims were compared"
        );
        judged += 1;
    }
    assert!(
        mid_text >= 1,
        "the published run must carry a supported verdict on a mid-text span"
    );
    assert!(judged >= 1, "the published run must carry a judged plan");
}
