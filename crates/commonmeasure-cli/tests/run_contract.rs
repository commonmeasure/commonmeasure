//! The published artefacts, asserted against the real binary.
//!
//! Everything here runs the `commonmeasure` binary as a process into a temporary directory and
//! reads what it wrote, because the defects that matter in this product are in
//! the emitted document rather than in any single function: a run that claims a
//! verification state it never earned, or a licence nobody declared, is a worse
//! defect than a crash.
//!
//! Provider credentials are removed from the child environment rather than
//! merely left unset, so this suite can never make a billable call even if a
//! developer has exported a key.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

mod contract_keys;
mod credential_sweep;

use contract_keys::{documented_keys, keys_of};

/// Verification states from `docs/contracts/provider.md`, spelled out so a
/// drift to `live_verified` fails here rather than silently in a reader's eye.
const VERIFICATION_STATES: [&str; 6] = [
    "planned",
    "fixture-tested",
    "replay-tested",
    "spec-verified",
    "live-verified",
    "production-observed",
];

const PLAN_STATUSES: [&str; 3] = ["completed", "refused", "unavailable"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/commonmeasure-cli sits two levels below the repository root")
        .to_path_buf()
}

fn commonmeasure(output: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_commonmeasure"));
    command
        .current_dir(repo_root())
        .args(["run", "demo/jobs/energy-price-cap.json", "--output"])
        .arg(output)
        .env_remove("EXA_API_KEY")
        .env_remove("TAVILY_API_KEY")
        .env_remove("FIRECRAWL_API_KEY")
        .env_remove("TOLLBIT_API_KEY")
        .env_remove("COMMONMEASURE_INFERENCE_ENDPOINT");
    command
}

struct Run {
    directory: tempfile::TempDir,
    summary: Value,
}

impl Run {
    fn execute() -> Self {
        let directory = tempfile::tempdir().expect("tempdir");
        let output = directory.path().join("latest");
        let result = commonmeasure(&output)
            .output()
            .expect("the binary should start");
        assert!(
            result.status.success(),
            "run failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let summary = std::fs::read(output.join("summary.json")).expect("summary.json");
        Self {
            summary: serde_json::from_slice(&summary).expect("summary.json should be valid JSON"),
            directory,
        }
    }

    fn output(&self) -> PathBuf {
        self.directory.path().join("latest")
    }

    fn plans(&self) -> &Vec<Value> {
        self.summary["plans"].as_array().expect("plans")
    }

    fn manifest(&self) -> Value {
        let bytes = std::fs::read(self.output().join("manifest.json")).expect("manifest.json");
        let file: Value =
            serde_json::from_slice(&bytes).expect("manifest.json should be valid JSON");
        file["manifest"].clone()
    }
}

/// The summary, the selection and the manifest carry exactly the keys the
/// contract enumerates — no more, so nothing ships undocumented, and no
/// fewer, so the document cannot promise a field the run does not write.
///
/// This run declares no fetch target, no fidelity judge and no output
/// provenance, so the manifest carries none of the three conditional keys.
#[test]
fn the_published_artefacts_carry_exactly_the_documented_keys() {
    let run = Run::execute();

    assert_eq!(
        keys_of(&run.summary),
        documented_keys("Summary keys:"),
        "summary.json's top-level keys and the contract's enumeration disagree"
    );
    assert_eq!(
        keys_of(&run.summary["selection"]),
        documented_keys("Selection keys:"),
        "the selection record and the contract's enumeration disagree"
    );
    assert_eq!(
        keys_of(&run.manifest()),
        documented_keys("Manifest keys:"),
        "the sealed manifest and the contract's enumeration disagree"
    );
    for conditional in documented_keys("Manifest keys present only where the suite declares them:")
    {
        assert!(
            run.manifest().get(&conditional).is_none(),
            "this suite declares no `{conditional}`, so the manifest must not carry the key"
        );
    }
}

#[test]
fn the_published_summary_matches_the_documented_contract() {
    let run = Run::execute();

    for key in documented_keys("Summary keys:") {
        assert!(!run.summary[&key].is_null(), "summary is missing `{key}`");
    }
    assert_eq!(run.summary["schema_version"], "contextops-run/v7");
    assert!(!run.plans().is_empty(), "a run must produce plans");

    for plan in run.plans() {
        let id = plan["id"].as_str().unwrap_or("<unnamed>");
        for key in [
            "id",
            "provider",
            "verification_state",
            "status",
            "eligibility",
            "acquisition",
            "latency_ms",
            "sources",
            "source_count",
            "context_tokens_admitted",
            "answer",
            "inference",
            "policy_decisions",
            "processors",
            "gaps",
            "evaluation",
        ] {
            assert!(
                plan.get(key).is_some(),
                "plan `{id}` is missing the documented key `{key}`"
            );
        }
        assert!(
            VERIFICATION_STATES.contains(&plan["verification_state"].as_str().unwrap_or_default()),
            "plan `{id}` has a verification state outside the provider contract"
        );
        assert!(
            PLAN_STATUSES.contains(&plan["status"].as_str().unwrap_or_default()),
            "plan `{id}` has an unknown status"
        );
        // v4: the evaluation is one section per installed evaluator, each
        // carrying its own identity.
        for section in ["grounding", "coverage", "freshness"] {
            assert_eq!(
                plan["evaluation"][section]["evaluator"]["name"], section,
                "plan `{id}` is missing the documented evaluation section `{section}`"
            );
        }
    }

    for artefact in ["manifest.json", "evidence.ndjson", "summary.json"] {
        assert!(
            run.output().join(artefact).exists(),
            "{artefact} is missing from the published run"
        );
    }
}

/// The invariant that makes the rest of the record worth reading: a run with no
/// credentials and no gateway must claim nothing. No answer, no verification
/// state above `planned`, no citation verdict — only an evaluation record
/// stating nothing was checkable — and a gap explaining every absence.
#[test]
fn a_run_with_no_dependencies_claims_nothing_and_explains_every_absence() {
    let run = Run::execute();

    assert_eq!(run.summary["run"]["mode"], "no-external-acquisition");
    assert_eq!(run.summary["model_plan"]["configured"], false);

    for plan in run.plans() {
        let id = plan["id"].as_str().unwrap_or("<unnamed>");
        assert_eq!(
            plan["status"], "unavailable",
            "plan `{id}` reported success with nothing configured"
        );
        assert!(plan["answer"].is_null(), "plan `{id}` produced an answer");
        assert_eq!(
            plan["verification_state"], "planned",
            "plan `{id}` claims verification it did not earn"
        );
        assert!(
            plan["evaluation"]["grounding"]["unevaluated"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty()),
            "plan `{id}` must be unevaluated with the reason stated: nothing here was checkable"
        );
        assert_eq!(
            plan["evaluation"]["grounding"]["verdict_counts"]["supported"], 0,
            "plan `{id}` claims a supported citation with nothing configured"
        );
        // The suite declares no rubric and no as_of, so the rankable
        // measures say unmeasured and carry no fraction — never zero.
        for section in ["coverage", "freshness"] {
            assert!(
                plan["evaluation"][section]["unmeasured"].as_str().is_some(),
                "plan `{id}`'s {section} must say unmeasured with nothing declared"
            );
            assert!(
                plan["evaluation"][section]["fraction"].is_null(),
                "plan `{id}` fabricated a {section} fraction from nothing"
            );
        }
        assert!(
            !plan["gaps"].as_array().map(Vec::is_empty).unwrap_or(true),
            "plan `{id}` is unavailable but records no gap explaining what is missing"
        );
        assert_eq!(plan["source_count"], 0);
    }

    assert!(
        run.summary["selection"]["plan_id"].is_null(),
        "nothing completed, so nothing may be selected"
    );
}

/// No provider probed on 1 August 2026 returns licence metadata. A run that
/// invented one would be inferring permission from accessibility, which
/// `docs/contracts/provider.md` forbids.
#[test]
fn no_source_claims_a_licence_nobody_declared() {
    let run = Run::execute();
    for plan in run.plans() {
        for source in plan["sources"].as_array().into_iter().flatten() {
            assert_eq!(
                source["licence"]["state"], "unknown",
                "a source claims a licence state no provider declared"
            );
        }
    }
}

#[test]
fn no_artefact_contains_a_credential_shaped_string() {
    let run = Run::execute();
    credential_sweep::no_artefact_carries_a_credential(
        &run.output(),
        &["summary.json", "manifest.json", "evidence.ndjson"],
    );
}

/// The evidence log describes exactly one run. Re-running into the same
/// directory publishes a new run; it never appends to the old one's log.
#[test]
fn rerunning_into_one_directory_replaces_rather_than_accumulates() {
    let directory = tempfile::tempdir().expect("tempdir");
    let output = directory.path().join("latest");

    assert!(commonmeasure(&output).status().expect("run").success());
    let first = std::fs::read_to_string(output.join("evidence.ndjson")).unwrap();
    assert!(commonmeasure(&output).status().expect("re-run").success());
    let second = std::fs::read_to_string(output.join("evidence.ndjson")).unwrap();

    assert_eq!(
        first.lines().count(),
        second.lines().count(),
        "the evidence log grew on a second run into the same directory"
    );
    // Staging and set-aside directories carry a per-run identifier, so the
    // check is that nothing beside the published run remains, not that two
    // fixed names are absent.
    let leftovers: Vec<String> = std::fs::read_dir(directory.path())
        .expect("the output's parent is readable")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name != "latest")
        .collect();
    assert!(
        leftovers.is_empty(),
        "a completed run leaves no staging or superseded directory behind: {leftovers:?}"
    );
}

/// A run must be reproducible from its manifest, which is the claim the sealed
/// hash makes and the reason it is a hash rather than a label.
#[test]
fn repeated_runs_agree_apart_from_run_identity() {
    let (mut first, mut second) = (Run::execute().summary, Run::execute().summary);
    assert_eq!(
        first["run"]["manifest_hash"], second["run"]["manifest_hash"],
        "the same suite produced two different manifest hashes"
    );
    // The declared volatile fields, and only these. The evidence log's digest
    // is volatile by construction: the log records the run's own identity and
    // timestamps, so two runs of one suite cannot hash to the same log and
    // should not be expected to. Its record *count* is not volatile, and is
    // compared like everything else.
    for summary in [&mut first, &mut second] {
        summary["run"]["id"] = Value::Null;
        summary["run"]["started_at"] = Value::Null;
        summary["evidence_log"]["sha256"] = Value::Null;
    }
    assert_eq!(
        first, second,
        "two runs of one suite disagreed on something other than run identity"
    );
    assert_eq!(
        first["evidence_log"]["records"], second["evidence_log"]["records"],
        "the same suite must produce the same number of evidence records"
    );
}

/// What the run displays as policy must be what the job declared, or the claim
/// that the limits are enforced is unverifiable from the artefact.
#[test]
fn displayed_constraints_are_derived_from_the_suite_not_restated() {
    let run = Run::execute();
    let suite: Value = serde_json::from_slice(
        &std::fs::read(repo_root().join("demo/jobs/energy-price-cap.json")).unwrap(),
    )
    .unwrap();

    let declared = suite["job"]["constraints"].as_array().unwrap();
    let declared_tokens = declared
        .iter()
        .find(|c| c["kind"] == "maximum_context_tokens")
        .expect("the suite declares a context-token cap")["tokens"]
        .clone();
    assert_eq!(
        run.summary["job"]["constraints"]["max_context_tokens"],
        declared_tokens
    );

    let declared_hosts: Vec<&str> = declared
        .iter()
        .filter(|c| c["kind"] == "allowed_source_host")
        .map(|c| c["host"].as_str().unwrap())
        .collect();
    let shown: Vec<&str> = run.summary["job"]["constraints"]["allowed_source_hosts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|host| host.as_str().unwrap())
        .collect();
    assert_eq!(shown, declared_hosts);
}

/// `inspect` reads only the published artefacts, so anything it prints a
/// reviewer can also read for themselves — and a run with nothing configured
/// gets its absences explained, not abbreviated.
#[test]
fn inspect_reports_the_run_from_its_artefacts_alone() {
    let run = Run::execute();
    let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["inspect"])
        .arg(run.output())
        .output()
        .expect("inspect should start");
    assert!(output.status.success());

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("energy-price-cap/v1"));
    assert!(text.contains("no-external-acquisition"));
    assert!(
        text.contains("no inference gateway is configured"),
        "an unconfigured gateway is stated plainly"
    );
    assert!(
        text.contains("COMMONMEASURE_INFERENCE_ENDPOINT"),
        "the gap names the variable that would change the outcome"
    );
    assert!(text.contains("no plan was selected"));
    // The citation contract: the legend is present, and the claims carry
    // pointers into the artefacts the reviewer holds.
    assert!(text.contains("every claim ends with the record it was read from"));
    assert!(text.contains("[s:/run/mode]"));
    assert!(text.contains("[s:/selection]"));
}
