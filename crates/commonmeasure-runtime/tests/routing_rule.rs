//! The frozen routing rule (`demo/jobs/commerce-routing/rule.json`) must be
//! exactly what its own cited evidence derives: for each class, the supply
//! condition with the strictly highest mean weighted objective over the
//! pre-registered fitting questions, computed from the committed fitting
//! runs' evaluation records. This pin re-derives the rule from those records
//! and re-hashes the rule text, and it holds the artefact's published
//! `fitted_from` table entry by entry against the same records —
//! so an edited route, a drifted or falsified figure, a wrong manifest
//! citation or a reworded rule text fails the offline gate instead of
//! surviving as a claim.

use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};

const CONDITIONS: [&str; 4] = ["product", "brand", "editorial", "mixed"];

fn read(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).expect("read committed artefact"))
        .expect("parse committed artefact")
}

/// Equal to the precision the figures are published at. A published figure
/// is a decimal literal and the derived one is the product of two others, so
/// agreement is asserted to a tolerance far below any measured difference
/// (the smallest real gap in these records is 0.05) and far above the last
/// place of a double.
fn agrees(published: Option<f64>, derived: f64) -> bool {
    published.is_some_and(|value| (value - derived).abs() <= 1e-9)
}

#[test]
#[cfg_attr(
    not(evidence_output),
    ignore = "needs the committed runs: output/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn the_frozen_rule_re_derives_from_the_committed_fitting_runs() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let jobs = repository.join("demo/jobs/commerce-routing");
    let runs = Path::new(env!("COMMONMEASURE_EVIDENCE_DIR")).join("output/commerce-routing");
    let split = read(&jobs.join("split.json"));
    let rule = read(&jobs.join("rule.json"));

    let rule_text = rule["rule_text"].as_str().expect("rule text");
    assert_eq!(
        rule["rule_text_digest"].as_str().expect("digest"),
        format!("sha256:{:x}", Sha256::digest(rule_text.as_bytes())),
        "the digest must be the hash of the rule text it sits beside"
    );

    for (class, sets) in split["split"].as_object().expect("registered split") {
        let fitting: Vec<&str> = sets["fitting"]
            .as_array()
            .expect("fitting set")
            .iter()
            .map(|question| question.as_str().expect("question id"))
            .collect();

        let mut means: Vec<(&str, f64)> = Vec::new();
        for condition in CONDITIONS {
            let published = &rule["fitted_from"][class][condition];
            let published_questions = published["questions"].as_array().expect("questions");
            assert_eq!(
                published_questions.len(),
                fitting.len(),
                "{class}/{condition}: fitted_from must cite every fitting question"
            );

            let mut total = 0.0;
            for (index, question) in fitting.iter().enumerate() {
                let run = runs.join(format!("{condition}-{question}"));
                let manifest = read(&run.join("manifest.json"));
                let objective = &manifest["manifest"]["job"]["objective"];
                let summary = read(&run.join("summary.json"));
                let plan = summary["plans"]
                    .as_array()
                    .expect("plans")
                    .iter()
                    .find(|plan| plan["provider"] != "none")
                    .expect("the measured plan");
                let coverage = plan["evaluation"]["coverage"]["fraction"]
                    .as_f64()
                    .expect("coverage");
                let freshness = plan["evaluation"]["freshness"]["fraction"]
                    .as_f64()
                    .expect("freshness");
                let score = objective["coverage"].as_f64().expect("weight") * coverage
                    + objective["freshness"].as_f64().expect("weight") * freshness;
                total += score;

                // The artefact's published evidence table must equal the
                // records it cites — figures and citation alike.
                let entry = &published_questions[index];
                assert_eq!(
                    entry["question"], **question,
                    "{class}/{condition}[{index}]"
                );
                for (field, derived) in [
                    ("coverage", coverage),
                    ("freshness", freshness),
                    ("objective", score),
                ] {
                    assert!(
                        agrees(entry[field].as_f64(), derived),
                        "{class}/{condition}/{question}: fitted_from's {field} \
                         ({:?}) must equal the run record's ({derived})",
                        entry[field]
                    );
                }
                assert_eq!(
                    entry["manifest_hash"], manifest["hash"],
                    "{class}/{condition}/{question}: fitted_from must cite the run's \
                     actual manifest hash"
                );
            }
            let mean = total / fitting.len() as f64;
            assert!(
                agrees(published["mean_objective"].as_f64(), mean),
                "{class}/{condition}: fitted_from's mean_objective ({:?}) must be the \
                 mean of its own questions ({mean})",
                published["mean_objective"]
            );
            means.push((condition, mean));
        }
        means.sort_by(|a, b| b.1.total_cmp(&a.1));

        assert!(
            means[0].1 > means[1].1,
            "{class}: a tie on the mean objective leaves the class unrouted"
        );
        assert_eq!(
            rule["routes"][class]["condition"].as_str().expect("route"),
            means[0].0,
            "{class}: the frozen route must be the fitting set's strict argmax"
        );
    }
}
