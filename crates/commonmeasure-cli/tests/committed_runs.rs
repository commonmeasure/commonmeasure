//! A committed run that declares a current format version carries the shape
//! the contract describes for that version.
//!
//! `docs/contracts/run-output.md` enumerates the manifest's key set and the
//! keys an inference record carries, and every committed run declares the
//! version those enumerations belong to. A run sealed before a key existed
//! goes on declaring the same version, so adding a key to the runtime makes
//! every committed run's declaration false until the run is republished.
//! Nothing but this test notices, because a sealed artefact reads perfectly
//! well as the object it is; what is wrong is what it says it is.

mod contract_keys;

use std::path::{Path, PathBuf};

use contract_keys::{documented_keys, keys_of, repo_root};
use serde_json::Value;

/// The run and manifest versions the contract describes. Read from the
/// runtime rather than restated, so a version that moves moves here too.
const CURRENT_RUN_VERSION: &str = commonmeasure_runtime::SCHEMA_VERSION;

/// Committed runs sealed before a key the contract now enumerates, with what
/// republishes each family and why it has not been republished here.
///
/// Each entry must still name at least one run that fails the check below, so
/// republishing a family fails this test until its row goes. The list only
/// ever shrinks.
const AWAITING_REPUBLISH: &[(&str, &str)] = &[
    (
        "demo/output/skills",
        "`just skills-example`, which needs the two catalogued bundles installed where \
         `demo/skills/catalogue.json` says and a `/usr/bin/python3` that can import `yaml`: \
         both validators require it, and without it each exits with an import error, seals an \
         empty result and covers nothing",
    ),
    (
        "demo/output/commerce-routing/",
        "a new routing experiment, not a republish: `demo/jobs/commerce-routing/rule.json` is \
         frozen after the fitting runs and before the holdout and cites each fitting run by \
         manifest hash, so rebuilding those runs alone breaks the citation the experiment rests \
         on. Fitting, freezing a rule and running the holdout are one act",
    ),
];

/// The published run directories committed to this repository.
fn committed_runs(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect(&root.join("demo/output"), &mut found);
    found.sort();
    assert!(
        found.len() > 50,
        "the scan must reach the committed runs; found {}",
        found.len()
    );
    found
}

fn collect(dir: &Path, found: &mut Vec<PathBuf>) {
    // `.gitignore` keeps `demo/output/live-*/` out of the repository, so a
    // live run is a local artefact of whoever produced it and not a committed
    // run. Walking it would fail this test on the machine that has one.
    if dir
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("live-"))
    {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    if dir.join("summary.json").is_file() {
        found.push(dir.to_path_buf());
        return;
    }
    for entry in entries.filter_map(Result::ok) {
        if entry.path().is_dir() {
            collect(&entry.path(), found);
        }
    }
}

fn read(path: &Path) -> Value {
    let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// What a run declaring a current version fails to carry, or nothing.
fn shortfall(run: &Path, manifest_version: &str) -> Vec<String> {
    let mut missing = Vec::new();
    let manifest = read(&run.join("manifest.json"))["manifest"].clone();
    if manifest["manifest_version"] == manifest_version {
        let documented = documented_keys("Manifest keys:");
        let conditional =
            documented_keys("Manifest keys present only where the suite declares them:");
        let present: Vec<String> = keys_of(&manifest)
            .into_iter()
            .filter(|key| !conditional.contains(key))
            .collect();
        for key in documented.iter().filter(|key| !present.contains(key)) {
            missing.push(format!("manifest key `{key}`"));
        }
        for key in present.iter().filter(|key| !documented.contains(key)) {
            missing.push(format!("undocumented manifest key `{key}`"));
        }
    }

    let summary = read(&run.join("summary.json"));
    if summary["schema_version"] == CURRENT_RUN_VERSION {
        let documented = documented_keys("Inference keys:");
        for plan in summary["plans"].as_array().into_iter().flatten() {
            if !plan["inference"].is_object() {
                continue;
            }
            let present = keys_of(&plan["inference"]);
            for key in documented.iter().filter(|key| !present.contains(key)) {
                missing.push(format!("inference key `{key}`"));
            }
            for key in present.iter().filter(|key| !documented.contains(key)) {
                missing.push(format!("undocumented inference key `{key}`"));
            }
        }
    }
    missing.sort();
    missing.dedup();
    missing
}

#[test]
fn a_committed_run_declaring_a_current_version_carries_that_version_s_keys() {
    let root = repo_root();
    // The manifest version the contract's enumerations describe, taken from
    // the manifest the binary publishes now rather than restated here.
    let manifest_version = {
        let latest = read(&root.join("demo/output/latest/manifest.json"));
        latest["manifest"]["manifest_version"]
            .as_str()
            .expect("the republished example declares its manifest version")
            .to_owned()
    };

    let mut failures = Vec::new();
    let mut covered = vec![false; AWAITING_REPUBLISH.len()];
    for run in committed_runs(&root) {
        let relative = run
            .strip_prefix(&root)
            .expect("a run under the repository root")
            .to_string_lossy()
            .into_owned();
        let missing = shortfall(&run, &manifest_version);
        let awaiting = AWAITING_REPUBLISH
            .iter()
            .position(|(prefix, _)| relative.starts_with(prefix));
        match (missing.is_empty(), awaiting) {
            (true, _) => {}
            (false, Some(index)) => covered[index] = true,
            (false, None) => failures.push(format!("{relative} is missing {}", missing.join(", "))),
        }
    }
    assert!(
        failures.is_empty(),
        "a committed run declares a current format version and does not carry its keys. \
         Republish it with the recipe that builds it, or add it to AWAITING_REPUBLISH with \
         what republishes it:\n{}",
        failures.join("\n")
    );

    let stale: Vec<&str> = AWAITING_REPUBLISH
        .iter()
        .zip(&covered)
        .filter(|(_, still_short)| !**still_short)
        .map(|((prefix, _), _)| *prefix)
        .collect();
    assert!(
        stale.is_empty(),
        "these are republished and no longer short; delete their AWAITING_REPUBLISH rows:\n{}",
        stale.join("\n")
    );
}

/// The republished example is the one committed run that must always be
/// current: it is what `docs/contracts/run-output.md` points a first reader
/// at, and `just rubric-example` rebuilds it offline, so nothing excuses it
/// being behind.
#[test]
fn the_committed_example_carries_the_current_shape() {
    let root = repo_root();
    let latest = root.join("demo/output/latest");
    let manifest = read(&latest.join("manifest.json"));
    let manifest_version = manifest["manifest"]["manifest_version"]
        .as_str()
        .expect("a manifest version");
    assert_eq!(
        read(&latest.join("summary.json"))["schema_version"],
        CURRENT_RUN_VERSION
    );
    assert!(
        shortfall(&latest, manifest_version).is_empty(),
        "run `just rubric-example` to republish it: {:?}",
        shortfall(&latest, manifest_version)
    );
}
