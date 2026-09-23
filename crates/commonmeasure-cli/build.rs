//! Sets a `cfg` for each recorded fixture directory the maintainer supplies.
//!
//! The recorded fixtures (third-party provider responses, committed runs and
//! recorded host sessions) are not part of this repository. A maintainer who
//! holds them points `COMMONMEASURE_PRIVATE_EVIDENCE` at the directory that
//! contains them (`CONTRIBUTING.md`). A test that reads one carries
//! `#[cfg_attr(not(<cfg>), ignore = "…")]`, so a checkout without the
//! variable reports it as ignored, with the directory named, instead of
//! failing on a missing file.
//!
//! The resolved directory reaches the tests as `COMMONMEASURE_EVIDENCE_DIR`,
//! empty when the variable is unset, so that a test reads the same directory
//! the `cfg` was decided on.
//!
//! A present directory is watched through one file that rarely changes, so
//! that editing its contents does not rebuild the crate; an absent one is not
//! watched, because Cargo reruns a build script on every build while a
//! watched path is missing.

use std::path::Path;

/// The variable naming the fixture directory.
const VARIABLE: &str = "COMMONMEASURE_PRIVATE_EVIDENCE";

/// `(cfg, directory, watched file)`, relative to the fixture directory.
const FIXTURES: &[(&str, &str, &str)] = &[
    ("evidence_recon", "recon", "recon/README.md"),
    ("evidence_output", "output", "output/replay/manifest.json"),
    (
        "evidence_host_sessions",
        "host-sessions",
        "host-sessions/README.md",
    ),
];

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed={VARIABLE}");
    // A relative value is read from the repository root, where `just` runs.
    let root = std::env::var_os(VARIABLE)
        .filter(|value| !value.is_empty())
        .map(|value| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(value)
        })
        .map(|path| path.canonicalize().unwrap_or(path));
    println!(
        "cargo::rustc-env=COMMONMEASURE_EVIDENCE_DIR={}",
        root.as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default()
    );
    for (cfg, directory, watched) in FIXTURES {
        println!("cargo::rustc-check-cfg=cfg({cfg})");
        let Some(root) = root.as_ref() else { continue };
        if root.join(directory).is_dir() {
            println!("cargo::rustc-cfg={cfg}");
            let watched = root.join(watched);
            if watched.is_file() {
                println!("cargo::rerun-if-changed={}", watched.display());
            }
        }
    }
}
