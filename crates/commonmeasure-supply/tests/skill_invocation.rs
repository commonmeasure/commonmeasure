//! The skill adapter against real bundles on disk, executing real programs.
//!
//! There is no recorded form of a produced result and no loopback origin to
//! serve one from: a skill's answer exists only when something runs. So every
//! test here spawns a real child through the real adapter, and what is
//! purpose-written for the test is the *supply* — probe bundles standing in
//! the position `demo/corpus/` stands in for the internal adapter, never a
//! substitute for the adapter, the containment check, the limits or the
//! record.
//!
//! The probes are `/bin/sh` scripts because `sh` is present wherever this
//! gate runs and needs nothing installed. Which language a skill is written
//! in is the operator's declaration, not this adapter's business; declaring
//! an absolute interpreter and an explicit argument vector is the same act
//! whether it names `sh` or `python3`, and no shell ever parses the command
//! line this adapter builds. The two real third-party bundles are exercised
//! by the live test at the end, which is `#[ignore]`d because they are not in
//! this repository.

use std::path::Path;

use commonmeasure_supply::{LocalSkillAdapter, SkillCatalogue, SupplyAdapter, SupplyError};
use commonmeasure_types::{LicenceState, ProviderCapability};
use serde_json::Value;

/// A bundle: a `SKILL.md` declaring an identity, and an entrypoint.
fn bundle(root: &Path, frontmatter: &str, entrypoint: &str, script: &str) {
    std::fs::create_dir_all(root.join("scripts")).expect("mkdir");
    std::fs::write(root.join("SKILL.md"), frontmatter).expect("write SKILL.md");
    let path = root.join(entrypoint);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(&path, script).expect("write entrypoint");
}

const FRONTMATTER: &str = "---\nname: probe-skill\nlicense: Apache-2.0\nmetadata:\n  \
                           name: not-the-skills-name\n---\n\n# Probe\n";

fn catalogue_json(root: &Path, entry: &str) -> String {
    format!(
        r#"{{"skills": {{"probe": {{"root": {:?}, {entry}}}}}}}"#,
        root.display()
    )
}

/// Write a catalogue naming one probe skill and load it, returning the
/// adapter the runtime would build.
fn adapter(directory: &Path, root: &Path, entry: &str) -> Result<LocalSkillAdapter, SupplyError> {
    let path = directory.join("skills.json");
    std::fs::write(&path, catalogue_json(root, entry)).expect("write catalogue");
    let catalogue = SkillCatalogue::load(&path)?;
    let declaration = catalogue.skills.get("probe").expect("declared").clone();
    Ok(LocalSkillAdapter::new(&path, "probe", declaration))
}

const RESULT_ENTRY: &str = r#""entrypoint": "scripts/verdict.sh",
     "interpreter": "/bin/sh",
     "arguments": ["--target", "{job}"],
     "timeout_ms": 10000,
     "maximum_output_bytes": 65536"#;

#[test]
fn an_invocation_returns_the_produced_result_with_its_identity_and_command_line() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("probe-skill");
    bundle(
        &root,
        FRONTMATTER,
        "scripts/verdict.sh",
        "#!/bin/sh\nprintf 'verdict for %s: publishable\\n' \"$2\"\n",
    );
    let adapter = adapter(directory.path(), &root, RESULT_ENTRY).expect("catalogue loads");
    assert_eq!(adapter.provider(), "skill:probe");
    assert_eq!(adapter.capabilities(), &[ProviderCapability::Invoke]);

    let acquisition = adapter.invoke("bundles/target").expect("invocation runs");
    assert_eq!(acquisition.capability, ProviderCapability::Invoke);
    // No HTTP happened, so there is no status code to report; an exit code is
    // not one.
    assert_eq!(acquisition.http_status, None);
    // Nobody priced a local execution: unknown, never zero.
    assert_eq!(acquisition.charge.money, None);
    assert_eq!(acquisition.charge.native, None);

    // One invocation, one result, derived from the sealed response.
    assert_eq!(acquisition.envelopes.len(), 1);
    let envelope = &acquisition.envelopes[0];
    assert_eq!(
        envelope.text.as_deref(),
        Some("verdict for bundles/target: publishable\n")
    );
    // A file:// URL has no host, and nothing published or dated a produced
    // result. The skill's own `license` licenses the procedure, not the
    // output, so the envelope's licence stays unknown.
    assert!(envelope.source_url.starts_with("file://"));
    assert_eq!(envelope.host, "");
    assert_eq!(envelope.declared_date, None);
    assert_eq!(envelope.licence, LicenceState::Unknown);
    assert_eq!(envelope.retrieval_rank, 1);

    let invocation = acquisition
        .invocation
        .as_ref()
        .expect("invocation recorded");
    // The supplier's own declaration, taken from SKILL.md's top level: the
    // nested metadata block's `name` is somebody else's field.
    assert_eq!(invocation.declared_name.as_deref(), Some("probe-skill"));
    assert_eq!(invocation.declared_licence.as_deref(), Some("Apache-2.0"));
    // The format declares no version, so none is recorded — and the absence
    // is in the artefact rather than only in the prose.
    assert_eq!(invocation.declared_version, None);
    assert_eq!(invocation.exit_code, Some(0));
    assert_eq!(invocation.termination, "exited");

    // The command line verbatim, with one provenance per element: the
    // operator declared everything except the job's own value.
    assert_eq!(invocation.argv[0], "/bin/sh");
    assert!(invocation.argv[1].ends_with("scripts/verdict.sh"));
    assert_eq!(invocation.argv[2], "--target");
    assert_eq!(invocation.argv[3], "bundles/target");
    let sources = serde_json::to_value(&invocation.argv_source).expect("serialises");
    assert_eq!(
        sources,
        serde_json::json!(["operator", "operator", "operator", "job"])
    );

    // The sealed response is what the envelope was derived from, so a
    // reviewer re-derives the parse rather than trusting it.
    let sealed: Value = serde_json::from_slice(&acquisition.raw_response).expect("sealed JSON");
    assert_eq!(
        sealed["result"]["text"],
        json_text(envelope.text.as_deref())
    );
    assert_eq!(
        sealed["result"]["content_hash"],
        json_text(envelope.content_hash.as_deref())
    );
    assert_eq!(sealed["invocation"]["exit_code"], 0);
}

fn json_text(value: Option<&str>) -> Value {
    value.map(Value::from).unwrap_or(Value::Null)
}

#[test]
fn a_non_zero_exit_is_a_recorded_result_and_not_a_run_error() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("probe-skill");
    bundle(
        &root,
        FRONTMATTER,
        "scripts/verdict.sh",
        "#!/bin/sh\necho 'validation failed: missing manifest'\nexit 1\n",
    );
    let adapter = adapter(directory.path(), &root, RESULT_ENTRY).expect("catalogue loads");

    // The verdict *is* the answer the job asked for. A validator that exits
    // non-zero has done its work, and recording that as a supply error would
    // throw away the finding.
    let acquisition = adapter
        .invoke("bundles/target")
        .expect("a verdict, not an error");
    let invocation = acquisition
        .invocation
        .as_ref()
        .expect("invocation recorded");
    assert_eq!(invocation.exit_code, Some(1));
    assert_eq!(
        acquisition.envelopes[0].text.as_deref(),
        Some("validation failed: missing manifest\n")
    );
}

#[test]
fn an_entrypoint_resolving_outside_the_root_is_refused_unexecuted() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("probe-skill");
    let outside = directory.path().join("outside.sh");
    let marker = directory.path().join("executed.marker");
    bundle(
        &root,
        FRONTMATTER,
        "scripts/kept.sh",
        "#!/bin/sh\necho kept\n",
    );
    std::fs::write(
        &outside,
        format!("#!/bin/sh\ntouch {}\necho reached\n", marker.display()),
    )
    .expect("write outside script");
    std::os::unix::fs::symlink(&outside, root.join("scripts/escape.sh")).expect("symlink");

    let adapter = adapter(
        directory.path(),
        &root,
        r#""entrypoint": "scripts/escape.sh",
           "interpreter": "/bin/sh",
           "arguments": [],
           "timeout_ms": 10000,
           "maximum_output_bytes": 65536"#,
    )
    .expect("catalogue loads");

    match adapter.invoke("anything") {
        Err(SupplyError::Malformed { detail }) => {
            assert!(detail.contains("resolves outside"), "{detail}");
            assert!(detail.contains("was not executed"), "{detail}");
        }
        other => panic!("a symlink out of the root must be refused: {other:?}"),
    }
    // The refusal is before the spawn, not after it: the script outside the
    // root left no trace because it never ran.
    assert!(
        !marker.exists(),
        "the containment check must refuse before anything executes"
    );
}

#[test]
fn the_child_cannot_read_the_operators_environment() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("probe-skill");
    bundle(
        &root,
        FRONTMATTER,
        "scripts/verdict.sh",
        "#!/bin/sh\nprintf 'saw=%s\\n' \"${COMMONMEASURE_PROBE_SECRET-nothing}\"\n",
    );
    let adapter = adapter(directory.path(), &root, RESULT_ENTRY).expect("catalogue loads");

    // A provider credential in this process's environment is exactly what a
    // third party's code must not be able to read.
    unsafe { std::env::set_var("COMMONMEASURE_PROBE_SECRET", "a-credential-shaped-string") };
    let acquisition = adapter.invoke("bundles/target").expect("invocation runs");
    unsafe { std::env::remove_var("COMMONMEASURE_PROBE_SECRET") };

    assert_eq!(
        acquisition.envelopes[0].text.as_deref(),
        Some("saw=nothing\n")
    );
    let invocation = acquisition
        .invocation
        .as_ref()
        .expect("invocation recorded");
    assert!(
        invocation.environment.starts_with("cleared"),
        "{invocation:?}"
    );
}

#[test]
fn an_invocation_over_its_declared_timeout_is_killed_and_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("probe-skill");
    bundle(
        &root,
        FRONTMATTER,
        "scripts/verdict.sh",
        "#!/bin/sh\necho starting\nsleep 30\necho finished\n",
    );
    let adapter = adapter(
        directory.path(),
        &root,
        r#""entrypoint": "scripts/verdict.sh",
           "interpreter": "/bin/sh",
           "arguments": [],
           "timeout_ms": 200,
           "maximum_output_bytes": 65536"#,
    )
    .expect("catalogue loads");

    let started = std::time::Instant::now();
    match adapter.invoke("anything") {
        Err(SupplyError::Execution { detail }) => {
            assert!(detail.contains("200 ms"), "{detail}");
            assert!(detail.contains("killed"), "{detail}");
        }
        other => panic!("an overrunning invocation must be refused: {other:?}"),
    }
    // Enforced, not merely declared: the adapter returned long before the
    // child's own thirty seconds were up.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "the timeout must kill the child rather than wait for it"
    );
}

#[test]
fn a_result_over_the_declared_cap_is_refused_whole_and_never_truncated() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("probe-skill");
    bundle(
        &root,
        FRONTMATTER,
        "scripts/verdict.sh",
        "#!/bin/sh\nwhile :; do printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'; done\n",
    );
    let adapter = adapter(
        directory.path(),
        &root,
        r#""entrypoint": "scripts/verdict.sh",
           "interpreter": "/bin/sh",
           "arguments": [],
           "timeout_ms": 20000,
           "maximum_output_bytes": 4096"#,
    )
    .expect("catalogue loads");

    let started = std::time::Instant::now();
    match adapter.invoke("anything") {
        Err(SupplyError::Execution { detail }) => {
            assert!(detail.contains("4096-byte cap"), "{detail}");
            assert!(detail.contains("refused whole"), "{detail}");
        }
        other => panic!("an oversized result must be refused whole: {other:?}"),
    }
    // The cap kills the child at once rather than leaving it blocked on a
    // pipe until the far longer timeout: an oversized result must not
    // masquerade as a slow one.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "exceeding the cap must end the invocation, not wait out the timeout"
    );
}

#[test]
fn a_skill_that_reads_standard_input_gets_end_of_file_rather_than_hanging() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("probe-skill");
    bundle(
        &root,
        FRONTMATTER,
        "scripts/verdict.sh",
        "#!/bin/sh\nwhile read line; do echo \"read: $line\"; done\necho 'stdin closed'\n",
    );
    let adapter = adapter(
        directory.path(),
        &root,
        r#""entrypoint": "scripts/verdict.sh",
           "interpreter": "/bin/sh",
           "arguments": [],
           "timeout_ms": 5000,
           "maximum_output_bytes": 65536"#,
    )
    .expect("catalogue loads");

    let acquisition = adapter.invoke("anything").expect("invocation runs");
    assert_eq!(
        acquisition.envelopes[0].text.as_deref(),
        Some("stdin closed\n")
    );
}

#[test]
fn a_bundle_with_no_skill_md_declares_no_identity_and_is_not_executed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("probe-skill");
    let marker = directory.path().join("executed.marker");
    std::fs::create_dir_all(root.join("scripts")).expect("mkdir");
    std::fs::write(
        root.join("scripts/verdict.sh"),
        format!("#!/bin/sh\ntouch {}\n", marker.display()),
    )
    .expect("write entrypoint");

    let adapter = adapter(directory.path(), &root, RESULT_ENTRY).expect("catalogue loads");
    match adapter.invoke("anything") {
        Err(SupplyError::Malformed { detail }) => {
            assert!(detail.contains("SKILL.md"), "{detail}");
            assert!(detail.contains("nothing was executed"), "{detail}");
        }
        other => panic!("a bundle with no declared identity must not run: {other:?}"),
    }
    assert!(
        !marker.exists(),
        "nothing may execute before identity is read"
    );
}

#[test]
fn a_catalogue_that_could_not_be_honestly_executed_fails_to_load() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("probe-skill");
    bundle(
        &root,
        FRONTMATTER,
        "scripts/verdict.sh",
        "#!/bin/sh\necho hi\n",
    );

    // An interpreter resolved through PATH would be chosen by an environment
    // this adapter clears.
    let relative = adapter(
        directory.path(),
        &root,
        r#""entrypoint": "scripts/verdict.sh",
           "interpreter": "sh",
           "arguments": [],
           "timeout_ms": 1000,
           "maximum_output_bytes": 1024"#,
    );
    assert!(
        matches!(relative, Err(SupplyError::Malformed { ref detail }) if detail.contains("absolute path")),
        "relative loaded when it should not have"
    );

    // A job value spliced into operator text has two provenances and one
    // string to record them in.
    let spliced = adapter(
        directory.path(),
        &root,
        r#""entrypoint": "scripts/verdict.sh",
           "interpreter": "/bin/sh",
           "arguments": ["--target={job}"],
           "timeout_ms": 1000,
           "maximum_output_bytes": 1024"#,
    );
    assert!(
        matches!(spliced, Err(SupplyError::Malformed { ref detail }) if detail.contains("own argument")),
        "spliced loaded when it should not have"
    );

    // A limit of zero is not a limit.
    let unbounded = adapter(
        directory.path(),
        &root,
        r#""entrypoint": "scripts/verdict.sh",
           "interpreter": "/bin/sh",
           "arguments": [],
           "timeout_ms": 0,
           "maximum_output_bytes": 1024"#,
    );
    assert!(
        matches!(unbounded, Err(SupplyError::Malformed { ref detail }) if detail.contains("zero timeout_ms")),
        "unbounded loaded when it should not have"
    );

    // A misspelled key is a declaration lost, and the recovery would be to
    // execute something the operator did not name.
    let misspelled = adapter(
        directory.path(),
        &root,
        r#""entrypoint": "scripts/verdict.sh",
           "interpretor": "/bin/sh",
           "arguments": [],
           "timeout_ms": 1000,
           "maximum_output_bytes": 1024"#,
    );
    assert!(
        matches!(misspelled, Err(SupplyError::Malformed { ref detail }) if detail.contains("not a valid skill catalogue")),
        "misspelled loaded when it should not have"
    );
}

#[test]
fn a_skill_declares_invoke_and_refuses_every_capability_it_does_not_implement() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("probe-skill");
    bundle(
        &root,
        FRONTMATTER,
        "scripts/verdict.sh",
        "#!/bin/sh\necho hi\n",
    );
    let adapter = adapter(directory.path(), &root, RESULT_ENTRY).expect("catalogue loads");

    // Skill and content supply share one dispatch path and disjoint declared
    // capabilities. Nothing can ask a local execution for search semantics, a
    // bounded corpus's coverage, or a supplier's price.
    for outcome in [
        adapter.search("anything", 1, &[]),
        adapter.query("anything", 1),
    ] {
        assert!(
            matches!(outcome, Err(SupplyError::CapabilityUnavailable { .. })),
            "{outcome:?}"
        );
    }
    assert!(
        matches!(
            adapter.quote("anything", 1),
            Err(SupplyError::CapabilityUnavailable { .. })
        ),
        "no supplier prices a local execution, so quote must refuse"
    );
}

/// The two real third-party bundles, executed as installed.
///
/// Ignored, and this is the honest reason: the bundles are licensed
/// third-party content and are not copied into this repository, so this test
/// depends on a machine where they are installed. It is the live-execution
/// test — the committed run under `output/` is the acceptance evidence,
/// and the offline tests above prove the adapter, the containment, the
/// isolation and the limits without it.
///
/// Run it with the catalogue that names them:
/// `COMMONMEASURE_SKILL_CATALOGUE=demo/skills/catalogue.json cargo test -p
/// commonmeasure-supply --test skill_invocation -- --ignored`
#[test]
#[ignore = "executes third-party bundles installed on the operator's machine"]
fn the_two_real_skills_answer_the_same_input_with_different_verdicts() {
    // A test process runs in its crate directory, not the repository root, so
    // both the catalogue and the target are resolved against the root here.
    // The run path does not need this: a run's cwd is wherever the operator
    // invoked it, and the invocation record names it.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/commonmeasure-supply sits two levels below the repository root");
    let path = root.join(
        std::env::var("COMMONMEASURE_SKILL_CATALOGUE")
            .expect("COMMONMEASURE_SKILL_CATALOGUE names the operator's catalogue"),
    );
    let catalogue = SkillCatalogue::load(&path).expect("catalogue loads");
    let target = root.join("demo/skills/geo-lookup");
    let target = target.to_str().expect("a UTF-8 path");

    let mut verdicts = Vec::new();
    for name in ["skill-creator", "plugin-creator"] {
        let declaration = catalogue.skills.get(name).expect("declared").clone();
        let acquisition = LocalSkillAdapter::new(&path, name, declaration)
            .invoke(target)
            .expect("the bundle executes");
        let invocation = acquisition.invocation.expect("invocation recorded");
        verdicts.push((
            invocation.exit_code,
            acquisition.envelopes[0].text.clone().unwrap_or_default(),
        ));
    }

    // Identical input bytes, different procedures, different verdicts — which
    // is the whole claim: these are two routes to one job, not one route
    // twice.
    assert_ne!(verdicts[0], verdicts[1]);
    assert!(verdicts.iter().all(|(_, text)| !text.trim().is_empty()));
}
