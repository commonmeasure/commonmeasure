//! The governed admission path, end to end over the real internal corpus
//! adapter: a suite-declared rule set deciding, per source, what may reach a
//! model — with the refusal recorded before inference, not instead of it.
//!
//! The corpus is a real directory on disk read by the production adapter;
//! the gateway is a loopback origin that counts what it is asked. The count
//! is the evidence that a fully-refused plan really did stop before
//! inference (`demo/specialist/README.md`: an unentitled or deprecated-path request
//! is refused at admission, with the refusal recorded before inference).

use std::path::Path;
use std::sync::{Arc, Mutex};

use commonmeasure_http::{Response, Server, ServerHandle};
use commonmeasure_inference::TensorZeroBackend;
use commonmeasure_runtime::{RunOptions, Suite, execute};
use commonmeasure_supply::InternalCorpusAdapter;
use commonmeasure_types::PolicyMode;
use serde_json::{Value, json};
use uuid::Uuid;

fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(path, content).expect("write");
}

/// A corpus in the specialist bundle's shape: one supported document, one
/// deprecated-path document, one above the granted tier, one with no
/// declared metadata at all. Every document matches the job prompt, so what
/// differs between them is governance alone.
fn corpus() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    write(
        root,
        "corpus.json",
        r#"{"name": "governed test corpus",
            "licence": {"state": "declared", "reference": "test/licence-v1"},
            "dates": {
              "supported.md": "2026-05-01",
              "deprecated.md": "2026-03-01",
              "premier.md": "2026-04-01"
            },
            "documents": {
              "supported.md": {
                "edition": "orchestrator", "version_range": "4.x",
                "integration_path": "stream-gateway",
                "support_status": "supported", "entitlement": "standard"},
              "deprecated.md": {
                "edition": "orchestrator", "version_range": "4.x",
                "integration_path": "flux-connector",
                "support_status": "deprecated", "entitlement": "standard"},
              "premier.md": {
                "edition": "orchestrator", "version_range": "4.x",
                "integration_path": "bulk-export",
                "support_status": "supported", "entitlement": "premier"}
            }}"#,
    );
    write(
        root,
        "supported.md",
        "# Stream gateway\n\nConnect a data source through the stream gateway.\n",
    );
    write(
        root,
        "deprecated.md",
        "# Flux connector\n\nConnect a data source through the flux connector.\n",
    );
    write(
        root,
        "premier.md",
        "# Bulk export\n\nConnect a data source export for premier customers.\n",
    );
    write(
        root,
        "undeclared.md",
        "# Overview\n\nWays to connect a data source, surveyed.\n",
    );
    directory
}

fn gateway_origin() -> (ServerHandle, Arc<Mutex<u32>>) {
    let calls = Arc::new(Mutex::new(0u32));
    let counter = Arc::clone(&calls);
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |_| {
            *counter.lock().expect("counter") += 1;
            Response::json(
                200,
                r#"{"id":"tz-governed","model":"local-test-model",
                    "choices":[{"message":{"content":"Answered from the admitted context."}}],
                    "usage":{"prompt_tokens":40,"completion_tokens":6}}"#,
            )
        })
        .expect("spawn");
    (handle, calls)
}

fn governance() -> Value {
    json!({
        "name": "test-support-rules",
        "version": "1",
        "entitlement_tiers": ["standard", "premier"],
        "granted_entitlement": "standard",
        "rules": [
            {"edition": "orchestrator", "version_range": "4.x",
             "integration_path": "stream-gateway",
             "support_status": "supported", "effective_date": "2026-03-01"},
            {"edition": "orchestrator", "version_range": "4.x",
             "integration_path": "flux-connector",
             "support_status": "deprecated", "effective_date": "2026-03-01"},
            {"edition": "orchestrator", "version_range": "4.x",
             "integration_path": "bulk-export",
             "support_status": "supported", "effective_date": "2026-04-10"}
        ]
    })
}

fn suite(policy_mode: PolicyMode, governed: bool, prompt: &str) -> Suite {
    let mut suite = json!({
        "suite_version": "governed-test/v1",
        "label": "Governed admission path",
        "job": {
            "id": Uuid::new_v4(),
            "kind": "research.answer",
            "prompt": prompt,
            "policy_mode": policy_mode,
            "objective": {"kind": "minimise_latency"},
            "constraints": [],
            "evidence_requirements": []
        },
        "model_plan": {"name": "pinned", "version": "1", "model": "local-test-model"},
        "result_limit": 4,
        "providers": ["internal"],
        "as_of": {"date": "2026-08-06", "maximum_age_days": 365}
    });
    if governed {
        suite["governance"] = governance();
    }
    serde_json::from_value(suite).expect("a well-formed suite")
}

struct Harness {
    _gateway: ServerHandle,
    gateway_calls: Arc<Mutex<u32>>,
    _corpus: tempfile::TempDir,
    _directory: tempfile::TempDir,
    summary: Value,
}

impl Harness {
    fn run(suite: &Suite) -> Self {
        let corpus = corpus();
        let (gateway, gateway_calls) = gateway_origin();
        let directory = tempfile::tempdir().expect("tempdir");
        let root = corpus.path().to_path_buf();
        let options = RunOptions {
            // The committed comparison runs execute in no-external-acquisition
            // mode: an internal query still runs, because it bills nobody.
            allow_external_acquisition: false,
            output: directory.path().join("run"),
            suppliers: Box::new(move |_| {
                Ok(Box::new(InternalCorpusAdapter::new(&root))
                    as Box<dyn commonmeasure_supply::SupplyAdapter>)
            }),
            backend: Some(Box::new(
                TensorZeroBackend::new(gateway.url()).expect("a loopback gateway endpoint"),
            )),
            replay: None,
            allowance: None,
            provenance_signing:
                commonmeasure_runtime::processor::provenance::SigningIdentity::Unconfigured,
        };
        let summary = execute(suite, &options)
            .expect("the run should complete")
            .summary;
        Self {
            _gateway: gateway,
            gateway_calls,
            _corpus: corpus,
            _directory: directory,
            summary,
        }
    }

    fn plan(&self) -> &Value {
        self.summary["plans"]
            .as_array()
            .expect("plans")
            .iter()
            .find(|plan| plan["id"] == "internal-only")
            .expect("the internal plan")
    }

    fn source(&self, name: &str) -> &Value {
        self.plan()["sources"]
            .as_array()
            .expect("sources")
            .iter()
            .find(|source| {
                source["url"]
                    .as_str()
                    .is_some_and(|url| url.ends_with(name))
            })
            .unwrap_or_else(|| panic!("source {name} in the plan"))
    }

    fn governor_invocations(&self) -> Vec<&Value> {
        self.plan()["processors"]
            .as_array()
            .expect("processors")
            .iter()
            .filter(|record| record["processor"]["name"] == "support-governor")
            .collect()
    }

    fn gateway_calls(&self) -> u32 {
        *self.gateway_calls.lock().expect("counter")
    }
}

/// Every source the query returns is refused by governance — the deprecated
/// path, the premier document, the undeclared document — so no context
/// exists to answer from, and the gateway must never be asked.
#[test]
fn a_fully_refused_governed_plan_never_reaches_inference() {
    let harness = Harness::run(&suite(
        PolicyMode::Strict,
        true,
        // Matches the flux, premier and undeclared documents; not the
        // supported stream-gateway one.
        "flux connector export overview surveyed premier",
    ));
    let plan = harness.plan();

    assert_eq!(plan["status"], "unavailable");
    assert_eq!(plan["answer"], Value::Null);
    assert_eq!(
        harness.gateway_calls(),
        1,
        "only the baseline was inferred: a fully-refused plan must stop before \
         inference, not merely omit the answer"
    );
    // Each refusal is recorded per source, naming the rule set that decided.
    for name in ["deprecated.md", "premier.md", "undeclared.md"] {
        let source = harness.source(name);
        assert_eq!(source["admitted"], false, "{name}");
        let reason = source["admission_reason"].as_str().expect("a reason");
        assert!(
            reason.contains("test-support-rules/1"),
            "{name}: the reason must name the rule set: {reason}"
        );
    }
    // And each refusal is an invocation record, present before inference in
    // the append-only trail.
    let refusals: Vec<_> = harness
        .governor_invocations()
        .into_iter()
        .filter(|record| record["decision"] == "refuse")
        .collect();
    assert_eq!(refusals.len(), 3, "one refusal record per refused source");
}

/// The mixed case: the supported document is admitted and answered from;
/// the deprecated path and the unentitled document are refused beside it,
/// each with the reason class the scope sheet predeclares.
#[test]
fn a_governed_plan_admits_the_supported_path_and_refuses_the_rest() {
    let harness = Harness::run(&suite(
        PolicyMode::Strict,
        true,
        "connect a data source flux stream gateway export overview surveyed",
    ));
    let plan = harness.plan();

    assert_eq!(plan["status"], "completed");
    assert_eq!(plan["answer"], "Answered from the admitted context.");
    assert_eq!(
        harness.gateway_calls(),
        2,
        "the governed plan and the baseline; the refused sources bought no third call"
    );

    assert_eq!(harness.source("supported.md")["admitted"], true);
    let deprecated = harness.source("deprecated.md");
    assert_eq!(deprecated["admitted"], false);
    assert!(
        deprecated["admission_reason"]
            .as_str()
            .unwrap()
            .contains("deprecated"),
        "restraint: {}",
        deprecated["admission_reason"]
    );
    let premier = harness.source("premier.md");
    assert_eq!(premier["admitted"], false);
    assert!(
        premier["admission_reason"]
            .as_str()
            .unwrap()
            .contains("granted"),
        "entitlement: {}",
        premier["admission_reason"]
    );
    let undeclared = harness.source("undeclared.md");
    assert_eq!(undeclared["admitted"], false);
    assert!(
        undeclared["admission_reason"]
            .as_str()
            .unwrap()
            .contains("no governance metadata"),
        "unknown is not supported: {}",
        undeclared["admission_reason"]
    );
}

/// Observe mode records the identical breaches and carries on — the mode
/// decides what happens to a breach, never whether it is recorded.
#[test]
fn observe_mode_records_the_breaches_and_admits() {
    let harness = Harness::run(&suite(
        PolicyMode::Observe,
        true,
        "connect a data source flux stream gateway export overview surveyed",
    ));
    let plan = harness.plan();

    assert_eq!(plan["status"], "completed");
    for name in [
        "supported.md",
        "deprecated.md",
        "premier.md",
        "undeclared.md",
    ] {
        assert_eq!(harness.source(name)["admitted"], true, "{name}");
    }
    // The breaches are recorded identically to strict's refusals: one
    // invocation per source, three carrying gaps.
    let invocations = harness.governor_invocations();
    assert_eq!(invocations.len(), 4);
    let breaches = invocations
        .iter()
        .filter(|record| !record["gaps"].as_array().unwrap().is_empty())
        .count();
    assert_eq!(
        breaches, 3,
        "the same three breaches observe merely records"
    );
    assert!(
        invocations
            .iter()
            .all(|record| record["decision"] == "admit"),
        "observe admits with the breach recorded"
    );
}

/// An ungoverned suite over the same corpus records no governor invocation
/// at all: absence of a record and a record of absence are different facts.
#[test]
fn an_ungoverned_suite_records_no_governor_invocation() {
    let harness = Harness::run(&suite(
        PolicyMode::Strict,
        false,
        "connect a data source flux stream gateway export overview surveyed",
    ));
    assert_eq!(harness.plan()["status"], "completed");
    assert!(
        harness.governor_invocations().is_empty(),
        "no governance declared, no governor run"
    );
    for name in ["deprecated.md", "premier.md", "undeclared.md"] {
        assert_eq!(harness.source(name)["admitted"], true, "{name}");
    }
}

/// The committed revocation pair, run twice each over the committed bundle
/// with no gateway: window composition and evaluation records are
/// byte-identical across runs (the governor, the query, coverage and
/// freshness are pure functions of suite input and corpus content; answers
/// are excluded because none exist), and the pair's manifest hashes differ
/// in exactly the direction the revocation demonstration claims — the
/// migration document admitted under rule set 1, refused under rule set 2.
#[test]
fn the_committed_revocation_pair_is_deterministic_and_its_hashes_differ() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repository root")
        .to_path_buf();
    let corpus = root.join("demo/specialist/corpus");
    let run = |suite: &Suite| {
        let corpus = corpus.clone();
        let directory = tempfile::tempdir().expect("tempdir");
        let summary = execute(
            suite,
            &RunOptions {
                allow_external_acquisition: false,
                output: directory.path().join("run"),
                suppliers: Box::new(move |_| {
                    Ok(Box::new(InternalCorpusAdapter::new(&corpus))
                        as Box<dyn commonmeasure_supply::SupplyAdapter>)
                }),
                backend: None,
                replay: None,
                allowance: None,
                provenance_signing:
                    commonmeasure_runtime::processor::provenance::SigningIdentity::Unconfigured,
            },
        )
        .expect("the committed suite runs")
        .summary;
        let plan = summary["plans"]
            .as_array()
            .expect("plans")
            .iter()
            .find(|plan| plan["id"] == "internal-only")
            .expect("the internal plan")
            .clone();
        (summary["run"]["manifest_hash"].clone(), plan)
    };
    let migration_admitted = |plan: &Value| {
        plan["sources"]
            .as_array()
            .expect("sources")
            .iter()
            .find(|source| {
                source["url"]
                    .as_str()
                    .is_some_and(|url| url.ends_with("flux-connector-4x-migration.md"))
            })
            .expect("the migration document matches the restraint prompt")["admitted"]
            == true
    };

    let mut hashes = Vec::new();
    for (name, admitted) in [("revocation-pre", true), ("revocation-post", false)] {
        let suite = commonmeasure_runtime::load_suite(
            &root.join(format!("demo/jobs/specialist/{name}.json")),
        )
        .expect("the committed suite loads");
        let (first_hash, first_plan) = run(&suite);
        let (second_hash, second_plan) = run(&suite);
        assert_eq!(first_hash, second_hash, "{name}: one suite, one experiment");
        for section in ["sources", "evaluation", "processors"] {
            // Timestamps and latency are volatile by design; the decision
            // content is not. Compare with the volatile fields normalised
            // away, as replay_mode.rs does.
            let strip = |value: &Value| {
                let mut text = serde_json::to_string_pretty(&value[section]).expect("json");
                for key in ["timestamp", "started_at", "finished_at", "latency_ms"] {
                    let pattern = format!("\"{key}\":");
                    text = text
                        .lines()
                        .filter(|line| !line.trim_start().starts_with(&pattern))
                        .collect::<Vec<_>>()
                        .join("\n");
                }
                text
            };
            assert_eq!(
                strip(&first_plan),
                strip(&second_plan),
                "{name}: {section} must be byte-identical across two runs"
            );
        }
        assert_eq!(
            migration_admitted(&first_plan),
            admitted,
            "{name}: the migration document's admission is the pair's whole point"
        );
        hashes.push(first_hash);
    }
    assert_ne!(
        hashes[0], hashes[1],
        "flipping one rule's effective date must move the manifest hash"
    );
}

/// A governed suite without a reference date is refused before anything
/// runs: rules take effect at dates, and guessing the date would govern by
/// a calendar nobody declared.
#[test]
fn a_governed_suite_without_as_of_is_refused_before_anything_runs() {
    let mut governed = suite(PolicyMode::Strict, true, "anything");
    governed.as_of = None;
    let directory = tempfile::tempdir().expect("tempdir");
    let result = execute(
        &governed,
        &RunOptions {
            allow_external_acquisition: false,
            output: directory.path().join("run"),
            suppliers: Box::new(|_| {
                Err(commonmeasure_supply::SupplyError::Transport {
                    detail: "never reached".to_owned(),
                })
            }),
            backend: None,
            replay: None,
            allowance: None,
            provenance_signing:
                commonmeasure_runtime::processor::provenance::SigningIdentity::Unconfigured,
        },
    );
    match result {
        Ok(_) => panic!("a governed suite with no as_of must not run"),
        Err(error) => assert!(
            error.to_string().contains("as_of"),
            "the refusal names the missing declaration: {error}"
        ),
    }
}
