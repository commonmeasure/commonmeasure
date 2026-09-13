//! The whole path, end to end, with nothing standing in for our own code.
//!
//! Two loopback origins are started: one serving the bytes Exa actually
//! returned on 1 August 2026 (`demo/recon/exa/exa-search.json`), one serving an
//! OpenAI-compatible completion. Everything between them — the adapter, the
//! HTTP client, the parser, eligibility, admission, the budget arithmetic, the
//! inference call, the evidence log and the published run directory — is the
//! production implementation.
//!
//! These tests are where the claim "recorded bytes travel through the real
//! path" is either true or it is not.

use std::path::Path;
use std::sync::{Arc, Mutex};

use commonmeasure_http::{Response, Server, ServerHandle};
use commonmeasure_inference::TensorZeroBackend;
use commonmeasure_runtime::{CoverageRubric, RubricItem, RunOptions, Suite, execute};
use commonmeasure_supply::ExaAdapter;
use commonmeasure_types::{
    AccessAction, Constraint, ContextJob, HostPattern, Money, Objective, PolicyMode,
};
use serde_json::Value;
use uuid::Uuid;

fn recon_exa_body() -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/commonmeasure-runtime sits two levels below the repository root")
        .join("demo/recon/exa/exa-search.json");
    let capture: Value =
        serde_json::from_slice(&std::fs::read(path).expect("read the Exa capture")).unwrap();
    serde_json::to_vec(&capture["response"]["body"]).unwrap()
}

fn provider_origin(body: Vec<u8>) -> ServerHandle {
    Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |_| Response::new(200, body.clone()))
        .expect("spawn")
}

/// A gateway that answers, and counts what it was asked. The count is the
/// evidence that a refused plan really did stop before inference rather than
/// merely omitting the answer from its record.
fn gateway_origin() -> (ServerHandle, Arc<Mutex<u32>>) {
    let calls = Arc::new(Mutex::new(0u32));
    let counter = Arc::clone(&calls);
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |_| {
            *counter.lock().expect("counter") += 1;
            Response::json(
                200,
                r#"{"id":"tz-e2e","model":"local-test-model",
                    "choices":[{"message":{"content":"The supplied evidence does not establish a figure."}}],
                    "usage":{"prompt_tokens":40,"completion_tokens":11}}"#,
            )
        })
        .expect("spawn");
    (handle, calls)
}

fn suite(policy_mode: PolicyMode, constraints: Vec<Constraint>) -> Suite {
    Suite {
        suite_version: "test/v1".into(),
        label: "End-to-end path".into(),
        job: ContextJob {
            id: Uuid::new_v4(),
            kind: "research.answer".into(),
            prompt: "What do the supplied sources establish?".into(),
            policy_mode,
            objective: Objective::MinimiseLatency,
            constraints,
            evidence_requirements: vec![],
        },
        model_plan: commonmeasure_types::ModelPlan {
            name: "pinned".into(),
            version: "1".into(),
            model: "local-test-model".into(),
        },
        result_limit: 2,
        providers: vec!["exa".into()],
        require_cited_answer: false,
        coverage_rubric: None,
        as_of: None,
        governance: None,
        fetch_target: None,
        fidelity_judge: false,
        output_provenance: None,
    }
}

struct Harness {
    _provider: ServerHandle,
    _gateway: Option<ServerHandle>,
    gateway_calls: Arc<Mutex<u32>>,
    directory: tempfile::TempDir,
    summary: Value,
}

impl Harness {
    fn run(suite: &Suite, with_gateway: bool) -> Self {
        Self::run_against(suite, recon_exa_body(), with_gateway)
    }

    /// The same real path with a different provider body served from the
    /// loopback origin — how a test exercises content the recorded corpus
    /// deliberately does not contain, such as personal identifiers.
    fn run_against(suite: &Suite, body: Vec<u8>, with_gateway: bool) -> Self {
        let provider = provider_origin(body);
        let (gateway, gateway_calls) = gateway_origin();
        let base_url = provider.url();
        let directory = tempfile::tempdir().expect("tempdir");
        let output = directory.path().join("latest");

        let options = RunOptions {
            allow_external_acquisition: true,
            output,
            // A real adapter, aimed at a local origin serving recorded provider
            // bytes. The adapter, transport, parser and policy are unchanged.
            suppliers: Box::new(move |_provider| {
                Ok(Box::new(ExaAdapter::new(&base_url, "test-key"))
                    as Box<dyn commonmeasure_supply::SupplyAdapter>)
            }),
            backend: with_gateway.then(|| {
                Box::new(
                    TensorZeroBackend::new(gateway.url()).expect("a loopback gateway endpoint"),
                ) as Box<_>
            }),
            replay: None,
            allowance: None,
            provenance_signing:
                commonmeasure_runtime::processor::provenance::SigningIdentity::Unconfigured,
        };
        let report = execute(suite, &options).expect("the run should complete");
        Self {
            _provider: provider,
            _gateway: with_gateway.then_some(gateway),
            gateway_calls,
            directory,
            summary: report.summary,
        }
    }

    fn plan(&self, id: &str) -> &Value {
        self.summary["plans"]
            .as_array()
            .expect("plans")
            .iter()
            .find(|plan| plan["id"] == id)
            .unwrap_or_else(|| panic!("no plan {id}"))
    }

    fn published(&self) -> std::path::PathBuf {
        self.directory.path().join("latest")
    }

    fn gateway_calls(&self) -> u32 {
        *self.gateway_calls.lock().expect("counter")
    }

    fn gaps(&self, plan: &str) -> Vec<String> {
        self.plan(plan)["gaps"]
            .as_array()
            .expect("gaps")
            .iter()
            .map(|gap| gap["reason"].as_str().unwrap_or_default().to_owned())
            .collect()
    }
}

#[test]
fn recorded_provider_bytes_reach_a_real_answer_through_the_real_path() {
    let harness = Harness::run(&suite(PolicyMode::Strict, vec![]), true);
    let plan = harness.plan("exa-only");

    assert_eq!(plan["status"], "completed");
    assert_eq!(
        plan["verification_state"], "live-verified",
        "an authenticated call whose response is sealed is what live-verified means"
    );
    assert_eq!(plan["source_count"], 2);
    assert_eq!(plan["inference"]["executed_model"], "local-test-model");
    assert_eq!(plan["inference"]["input_tokens"], 40);
    assert_eq!(
        plan["answer"], "The supplied evidence does not establish a figure.",
        "the answer must be the gateway's, not this runtime's"
    );

    // The observed charge came from the recorded costDollars.total.
    assert_eq!(plan["acquisition"]["charge"]["money"]["micros"], 7_000);
    assert_eq!(plan["acquisition"]["charge"]["money"]["currency"], "USD");

    // The exact bytes are sealed beside the run, and every admitted source's
    // hash can be recomputed from them.
    let sealed = harness
        .published()
        .join(plan["acquisition"]["response_ref"].as_str().unwrap());
    let sealed_bytes = std::fs::read(&sealed).expect("the response should be sealed");
    assert_eq!(
        format!(
            "sha256:{:x}",
            <sha2::Sha256 as sha2::Digest>::digest(&sealed_bytes)
        ),
        plan["acquisition"]["response_hash"].as_str().unwrap()
    );
    let sealed_json: Value = serde_json::from_slice(&sealed_bytes).unwrap();
    let recorded_text = sealed_json["results"][0]["text"].as_str().unwrap();
    assert_eq!(
        format!(
            "sha256:{:x}",
            <sha2::Sha256 as sha2::Digest>::digest(recorded_text.as_bytes())
        ),
        plan["sources"][0]["content_hash"].as_str().unwrap(),
        "a reviewer must be able to re-derive the content hash from the sealed response"
    );

    // The baseline ran too, so "did the context help?" has a comparison.
    assert_eq!(harness.plan("no-context")["status"], "completed");
    assert_eq!(harness.gateway_calls(), 2);

    // Latency is observable for every completed plan, so this objective is
    // computable and the router does not abstain.
    assert_eq!(harness.summary["selection"]["plan_id"], "exa-only");
    assert!(harness.summary["run"]["evidence_complete"] == Value::Bool(true));
}

/// Strict source policy stops disallowed hosts before the model sees them. The
/// gateway call count is the proof: a refusal that still sent the context would
/// be a refusal in the record only.
#[test]
fn strict_source_policy_refuses_a_disallowed_host_before_inference() {
    let harness = Harness::run(
        &suite(
            PolicyMode::Strict,
            vec![Constraint::AllowedSourceHost {
                host: "www.ofgem.gov.uk".into(),
            }],
        ),
        true,
    );
    let plan = harness.plan("exa-only");

    assert_eq!(plan["status"], "unavailable");
    assert_eq!(plan["source_count"], 0);
    assert_eq!(
        plan["verification_state"], "live-verified",
        "the refusals do not undo the authenticated call the sealed bytes prove"
    );
    assert!(plan["answer"].is_null(), "a refused plan has no answer");
    assert!(
        harness
            .gaps("exa-only")
            .contains(&"policy_refused".to_owned())
    );

    let refusals: Vec<&Value> = plan["policy_decisions"]
        .as_array()
        .expect("decisions")
        .iter()
        .filter(|decision| decision["decision"] == "refuse")
        .collect();
    assert_eq!(refusals.len(), 2, "both discovered sources were refused");
    assert!(
        refusals[0]["source_url"].is_string(),
        "a source refusal names the source it refused"
    );

    // Only the baseline reached the gateway.
    assert_eq!(harness.gateway_calls(), 1);
}

/// The same breach under `observe` is carried, not hidden: the sources are
/// admitted and the breach is still on the record.
#[test]
fn observe_mode_carries_the_same_breach_it_does_not_lose_it() {
    let harness = Harness::run(
        &suite(
            PolicyMode::Observe,
            vec![Constraint::AllowedSourceHost {
                host: "www.ofgem.gov.uk".into(),
            }],
        ),
        true,
    );
    let plan = harness.plan("exa-only");

    assert_eq!(plan["status"], "completed");
    assert_eq!(
        plan["source_count"], 2,
        "observe admits the breaching source"
    );
    assert!(
        harness
            .gaps("exa-only")
            .contains(&"policy_refused".to_owned()),
        "the breach is recorded whether or not it was enforced"
    );
}

/// An ordered access rule through the real path: the first rule matching the
/// source's host decides, and a `refuse` under `strict` keeps the source out
/// of inference with a decision naming the rule. The exception written before
/// the domain refusal is honoured, so of the two recorded `europa.eu` sources
/// the one the exception names is admitted and the other is refused.
#[test]
fn a_strict_access_rule_refuses_the_hosts_it_matches_before_inference() {
    let harness = Harness::run(
        &suite(
            PolicyMode::Strict,
            vec![
                Constraint::AccessRule {
                    host: HostPattern::parse("digital-strategy.ec.europa.eu").unwrap(),
                    action: AccessAction::Allow,
                },
                Constraint::AccessRule {
                    host: HostPattern::parse("*.europa.eu").unwrap(),
                    action: AccessAction::Refuse,
                },
            ],
        ),
        true,
    );
    let plan = harness.plan("exa-only");
    assert_eq!(plan["status"], "completed");
    assert_eq!(
        plan["source_count"], 1,
        "the source the allow rule names was admitted and the other refused"
    );
    let refusals: Vec<&Value> = plan["policy_decisions"]
        .as_array()
        .expect("decisions")
        .iter()
        .filter(|decision| decision["decision"] == "refuse")
        .collect();
    assert_eq!(refusals.len(), 1, "{:?}", plan["policy_decisions"]);
    let reason = refusals[0]["reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("access rule 2 (*.europa.eu)"),
        "the refusal names the rule by position and pattern: {reason}"
    );
    assert!(
        refusals[0]["source_url"]
            .as_str()
            .is_some_and(|url| url.starts_with("https://ai-act-service-desk.ec.europa.eu/")),
        "the allow rule written first spared its host: {}",
        refusals[0]["source_url"]
    );
    assert!(
        harness
            .gaps("exa-only")
            .contains(&"policy_refused".to_owned())
    );
}

/// A licence required for one host is unmet by a supplier that declares none:
/// absent licence evidence is unknown, never permitted, and the source is
/// refused with the missing evidence named.
#[test]
fn a_require_licence_rule_refuses_a_source_that_declares_no_licence() {
    let harness = Harness::run(
        &suite(
            PolicyMode::Strict,
            vec![Constraint::AccessRule {
                host: HostPattern::parse("*").unwrap(),
                action: AccessAction::RequireLicence {
                    licence: "rsl:publisher/2026".into(),
                },
            }],
        ),
        true,
    );
    let plan = harness.plan("exa-only");
    assert_eq!(plan["status"], "unavailable");
    assert_eq!(plan["source_count"], 0);
    assert!(
        harness
            .gaps("exa-only")
            .contains(&"evidence_missing".to_owned()),
        "no supplier declared a licence, and that is missing evidence"
    );
    let refusal = plan["policy_decisions"]
        .as_array()
        .expect("decisions")
        .iter()
        .find(|decision| decision["decision"] == "refuse")
        .expect("a refusal");
    assert!(
        refusal["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("requires licence \"rsl:publisher/2026\""),
        "{refusal}"
    );
}

/// A charge above the job's cap refuses the content it bought. The money is
/// already spent; what policy still controls is whether the result may be used.
#[test]
fn a_charge_above_the_cap_refuses_the_content_it_bought() {
    let harness = Harness::run(
        &suite(
            PolicyMode::Strict,
            vec![Constraint::MaximumAcquisitionCost {
                amount: Money::new("USD", 1_000),
            }],
        ),
        true,
    );
    let plan = harness.plan("exa-only");

    assert_eq!(plan["status"], "refused");
    assert_eq!(plan["source_count"], 0);
    assert!(
        harness
            .gaps("exa-only")
            .contains(&"budget_exhausted".to_owned())
    );
    assert_eq!(harness.gateway_calls(), 1, "only the baseline was inferred");
    // The acquisition still happened and is still evidenced: the run records
    // what it spent even when it refuses to use the result.
    assert_eq!(plan["acquisition"]["http_status"], 200);
}

/// A source that will not fit is left out whole. A truncated source has a
/// content hash matching nothing, and a citation into text the model never saw.
#[test]
fn the_context_budget_drops_whole_sources_rather_than_truncating_them() {
    let harness = Harness::run(
        &suite(
            PolicyMode::Strict,
            vec![Constraint::MaximumContextTokens { tokens: 30 }],
        ),
        true,
    );
    let plan = harness.plan("exa-only");

    assert_eq!(plan["status"], "completed");
    assert_eq!(plan["source_count"], 1, "only the first source fitted");
    assert!(
        plan["context_tokens_admitted"].as_u64().unwrap() <= 30,
        "the admitted footprint must respect the declared budget"
    );
    assert!(
        harness
            .gaps("exa-only")
            .contains(&"budget_exhausted".to_owned())
    );
    let dropped = &plan["sources"][1];
    assert_eq!(dropped["admitted"], false);
    assert!(
        dropped["content_hash"].is_string(),
        "a dropped source keeps the hash of what it would have contributed"
    );
}

/// A source policy refuses never consumes the context budget. The recorded
/// bytes hold two sources: the first is 30 words on a disallowed host, the
/// second 28 words on the allowed one. With a 28-word budget the refused first
/// source must not spend the budget the admitted second source needs — a gap
/// claiming the budget was spent, beside context that never reached it, would
/// be two artefacts contradicting each other.
#[test]
fn a_refused_source_does_not_consume_the_budget_an_admitted_one_needs() {
    let harness = Harness::run(
        &suite(
            PolicyMode::Strict,
            vec![
                Constraint::AllowedSourceHost {
                    host: "digital-strategy.ec.europa.eu".into(),
                },
                Constraint::MaximumContextTokens { tokens: 28 },
            ],
        ),
        true,
    );
    let plan = harness.plan("exa-only");

    assert_eq!(plan["status"], "completed");
    assert_eq!(plan["source_count"], 1);
    assert_eq!(plan["context_tokens_admitted"], 28);
    assert_eq!(plan["sources"][0]["admitted"], false);
    assert_eq!(plan["sources"][1]["admitted"], true);
    assert!(
        !harness
            .gaps("exa-only")
            .contains(&"budget_exhausted".to_owned()),
        "nothing admissible was over budget, so no budget gap may be claimed"
    );
}

/// The transform-stage optimiser removes the block the two recorded sources
/// repeat and keeps both sources: fewer admitted tokens, nothing the answer
/// needs dropped, and a transformation record a reviewer can re-derive from
/// the sealed bytes.
#[test]
fn the_optimiser_reduces_admitted_tokens_without_dropping_a_source() {
    let harness = Harness::run(&suite(PolicyMode::Strict, vec![]), true);
    let plan = harness.plan("exa-only");

    assert_eq!(plan["status"], "completed");
    assert_eq!(plan["source_count"], 2, "no source was dropped");

    let invocations = plan["processors"].as_array().expect("processors");
    let optimiser = invocations
        .iter()
        .find(|invocation| invocation["processor"]["name"] == "context-optimiser")
        .expect("the transform stage ran");
    let detail = &optimiser["detail"];
    let tokens_in = detail["tokens_in"].as_u64().expect("tokens in");
    let tokens_out = detail["tokens_out"].as_u64().expect("tokens out");
    assert!(
        tokens_out < tokens_in,
        "the recorded sources repeat a block, so deduplication must reduce the footprint \
         ({tokens_out} >= {tokens_in})"
    );
    assert_eq!(
        plan["context_tokens_admitted"].as_u64().unwrap(),
        tokens_out,
        "what was admitted is what the transform produced"
    );

    // Re-derive the transformation from the sealed bytes: take the original
    // text, excise the recorded spans, and the hash must match. The record
    // proves itself against the artefacts, not against trust in the runtime.
    let sealed = harness
        .published()
        .join(plan["acquisition"]["response_ref"].as_str().unwrap());
    let sealed_json: Value =
        serde_json::from_slice(&std::fs::read(&sealed).expect("sealed response")).unwrap();
    let removal = &detail["removed_spans"][0];
    let original = sealed_json["results"]
        .as_array()
        .expect("results")
        .iter()
        .find(|result| result["url"] == removal["reference"])
        .expect("the transformed source is in the sealed response")["text"]
        .as_str()
        .expect("text");
    let mut rederived = String::new();
    let mut cursor = 0;
    for span in removal["spans"].as_array().expect("spans") {
        let start = span["start"].as_u64().unwrap() as usize;
        let end = span["end"].as_u64().unwrap() as usize;
        rederived.push_str(&original[cursor..start]);
        cursor = end;
    }
    rederived.push_str(&original[cursor..]);
    assert_eq!(
        format!(
            "sha256:{:x}",
            <sha2::Sha256 as sha2::Digest>::digest(rederived.as_bytes())
        ),
        removal["output_hash"].as_str().unwrap(),
        "excising the recorded spans from the sealed text must re-derive the output hash"
    );

    // The same invocations sit in the append-only log, not only in the plan.
    let events =
        commonmeasure_runtime::EvidenceLog::read(&harness.published().join("evidence.ndjson"))
            .expect("read the log");
    assert!(
        events
            .iter()
            .any(|record| record["event"] == "processor_invoked"
                && record["payload"]["processor"]["name"] == "context-optimiser"),
        "the invocation must be in the evidence log"
    );
}

/// A recorded Exa-shaped body whose second result carries a structured
/// personal identifier. Served through the real adapter, transport and
/// parser: only the origin's bytes differ from the recorded corpus, which
/// deliberately contains none.
fn body_with_pii() -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "requestId": "test-pii",
        "results": [
            {
                "url": "https://a.example/clean",
                "title": "Clean source",
                "text": "The cap is set quarterly by the regulator.",
            },
            {
                "url": "https://a.example/casework",
                "title": "Casework contact",
                "text": "Send the meter reading to casework.team@example.co.uk with your \
                         account reference.",
            }
        ],
    }))
    .expect("serialise")
}

/// A strict-mode PII hit blocks the source before the model sees it. The
/// clean source still crosses: the detector refuses sources, not plans.
#[test]
fn a_strict_pii_finding_refuses_the_source_before_inference() {
    let harness = Harness::run_against(&suite(PolicyMode::Strict, vec![]), body_with_pii(), true);
    let plan = harness.plan("exa-only");

    assert_eq!(plan["status"], "completed");
    assert_eq!(plan["source_count"], 1, "only the clean source crossed");
    assert_eq!(plan["sources"][1]["admitted"], false);
    assert!(
        plan["sources"][1]["admission_reason"]
            .as_str()
            .unwrap()
            .contains("PII detector"),
        "the source names the check that refused it"
    );
    let refusal = plan["policy_decisions"]
        .as_array()
        .expect("decisions")
        .iter()
        .find(|decision| decision["decision"] == "refuse")
        .expect("the refusal is a policy decision");
    assert_eq!(refusal["source_url"], "https://a.example/casework");
    assert!(
        harness
            .gaps("exa-only")
            .contains(&"policy_refused".to_owned())
    );

    let detector = plan["processors"]
        .as_array()
        .expect("processors")
        .iter()
        .find(|invocation| {
            invocation["processor"]["name"] == "pii-detector" && invocation["decision"] == "refuse"
        })
        .expect("the refusing invocation is recorded")
        .clone();
    assert_eq!(detector["detail"]["categories"]["email_address"], 1);
    assert!(
        !detector.to_string().contains("casework.team@example.co.uk"),
        "the identifier itself must never enter the evidence record"
    );

    // The refused text reached no model: the only context sent was the clean
    // source, and the baseline is the other call.
    assert_eq!(harness.gateway_calls(), 2);
    assert!(
        !plan["inference"].to_string().contains("casework.team"),
        "no fragment of the refused source may appear in the inference record"
    );
}

/// The same finding under `observe` is carried, not hidden: the source is
/// admitted and the finding is recorded identically.
#[test]
fn observe_mode_records_the_pii_finding_and_carries_the_source() {
    let harness = Harness::run_against(&suite(PolicyMode::Observe, vec![]), body_with_pii(), true);
    let plan = harness.plan("exa-only");

    assert_eq!(plan["status"], "completed");
    assert_eq!(plan["source_count"], 2, "observe admits the flagged source");
    assert!(
        harness
            .gaps("exa-only")
            .contains(&"policy_refused".to_owned()),
        "the finding is recorded whether or not it was enforced"
    );
    let detector = plan["processors"]
        .as_array()
        .expect("processors")
        .iter()
        .find(|invocation| {
            invocation["processor"]["name"] == "pii-detector"
                && invocation["detail"]["categories"]["email_address"] == 1
        })
        .expect("the finding is recorded");
    assert_eq!(
        detector["decision"], "admit",
        "observe carries the source; the record still holds the finding"
    );
}

/// With no gateway there is no answer, and the run says so rather than
/// producing one from somewhere else.
#[test]
fn without_a_gateway_every_plan_is_explicitly_unavailable() {
    let harness = Harness::run(&suite(PolicyMode::Strict, vec![]), false);

    for plan_id in ["no-context", "exa-only"] {
        let plan = harness.plan(plan_id);
        assert_eq!(plan["status"], "unavailable");
        assert!(plan["answer"].is_null());
        assert!(
            harness
                .gaps(plan_id)
                .contains(&"inference_unavailable".to_owned()),
            "{plan_id} must name the missing gateway"
        );
    }
    assert_eq!(harness.summary["model_plan"]["configured"], false);
    // Acquisition still happened and is still evidenced.
    assert_eq!(harness.plan("exa-only")["acquisition"]["result_count"], 2);
    assert_eq!(harness.gateway_calls(), 0);

    // Nothing completed, so the router selects nothing and says why.
    assert!(harness.summary["selection"]["plan_id"].is_null());
}

/// A corpus of operator-owned documents on real disk, standing beside the
/// loopback provider origin: internal supply as a first-class provider.
fn internal_corpus() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        directory.path().join("corpus.json"),
        r#"{"name": "operator kb",
            "licence": {"state": "declared", "reference": "operator-owned/kb-terms-v1"}}"#,
    )
    .expect("manifest");
    std::fs::write(
        directory.path().join("briefing.md"),
        "# Briefing\n\nThe supplied sources establish the review cadence.\n",
    )
    .expect("doc");
    std::fs::write(
        directory.path().join("notes.md"),
        "# Notes\n\nWhat the sources establish is recorded here.\n",
    )
    .expect("doc");
    directory
}

/// Run one suite comparing an open-web `search` plan with an internal `query`
/// plan, with the corpus at `corpus` and external acquisition as given.
fn comparison(corpus: &Path, live: bool, with_gateway: bool) -> Harness {
    let provider = provider_origin(recon_exa_body());
    let (gateway, gateway_calls) = gateway_origin();
    let base_url = provider.url();
    let corpus_root = corpus.to_path_buf();
    let directory = tempfile::tempdir().expect("tempdir");
    let output = directory.path().join("latest");

    let mut suite = suite(PolicyMode::Observe, vec![]);
    suite.providers = vec!["exa".into(), "internal".into()];
    suite.result_limit = 3;

    let options = RunOptions {
        allow_external_acquisition: live,
        output,
        suppliers: Box::new(move |provider| {
            Ok(match provider {
                "internal" => Box::new(commonmeasure_supply::InternalCorpusAdapter::new(
                    &corpus_root,
                )) as Box<dyn commonmeasure_supply::SupplyAdapter>,
                _ => Box::new(ExaAdapter::new(&base_url, "test-key")),
            })
        }),
        backend: with_gateway.then(|| {
            Box::new(TensorZeroBackend::new(gateway.url()).expect("a loopback gateway endpoint"))
                as Box<_>
        }),
        replay: None,
        allowance: None,
        provenance_signing:
            commonmeasure_runtime::processor::provenance::SigningIdentity::Unconfigured,
    };
    let report = execute(&suite, &options).expect("the run should complete");
    Harness {
        _provider: provider,
        _gateway: with_gateway.then_some(gateway),
        gateway_calls,
        directory,
        summary: report.summary,
    }
}

/// The internal-supply acceptance path: one job, an open-web plan and an
/// internal `query` plan side by side, and the corpus's shortfall against the
/// requested result count published as a first-class coverage gap.
#[test]
fn an_internal_query_plan_compares_with_an_open_web_plan_over_one_job() {
    let corpus = internal_corpus();
    let harness = comparison(corpus.path(), true, true);

    let web = harness.plan("exa-only");
    assert_eq!(web["status"], "completed");
    assert_eq!(web["capability"], "search");

    let internal = harness.plan("internal-only");
    assert_eq!(internal["status"], "completed");
    assert_eq!(
        internal["capability"], "query",
        "a corpus query must not be recorded as a web search"
    );
    assert_eq!(
        internal["verification_state"], "live-verified",
        "the call against the real supply completed and its response is sealed"
    );
    assert!(
        internal["acquisition"]["http_status"].is_null(),
        "no HTTP happened, so no status may be recorded"
    );
    assert_eq!(internal["source_count"], 2);
    assert!(
        internal["sources"][0]["url"]
            .as_str()
            .unwrap()
            .starts_with("file://")
    );
    assert_eq!(
        internal["sources"][0]["licence"]["state"], "declared",
        "the operator's written declaration reaches the published source record"
    );
    assert_eq!(
        internal["answer"], "The supplied evidence does not establish a figure.",
        "internal context reaches the same real inference path"
    );

    // The corpus held two matching documents against three requested: a
    // coverage fact about bounded supply, published as a first-class gap.
    assert!(
        harness
            .gaps("internal-only")
            .contains(&"coverage_gap".to_owned()),
        "where the corpus failed to cover the job must be on the record"
    );
    assert!(
        !harness
            .gaps("exa-only")
            .contains(&"coverage_gap".to_owned()),
        "an open-web search returning little says nothing about what the web holds"
    );

    // The sealed query response is inspectable like any provider's: the
    // reviewer can re-derive every envelope from it.
    let sealed = harness
        .published()
        .join(internal["acquisition"]["response_ref"].as_str().unwrap());
    let sealed_json: Value =
        serde_json::from_slice(&std::fs::read(&sealed).expect("sealed")).unwrap();
    assert_eq!(sealed_json["documents_scanned"], 2);
    assert_eq!(sealed_json["requested"], 3);

    // Baseline, web and internal all answered: the full comparison exists.
    assert_eq!(harness.gateway_calls(), 3);
}

/// The --live gate is about money leaving the operator. A corpus query reads
/// supply the operator already owns, so the internal heatmap exists in the
/// offline mode, beside open-web plans that honestly report they did not run.
#[test]
fn the_internal_corpus_runs_without_live_authorisation_and_the_open_web_does_not() {
    let corpus = internal_corpus();
    let harness = comparison(corpus.path(), false, true);

    assert_eq!(harness.plan("exa-only")["status"], "unavailable");
    assert!(
        harness
            .gaps("exa-only")
            .contains(&"evidence_missing".to_owned())
    );
    let internal = harness.plan("internal-only");
    assert_eq!(
        internal["status"], "completed",
        "reading supply the operator owns needs no external authorisation"
    );
    assert_eq!(internal["source_count"], 2);
}

/// A plan whose acquisition never ran is not a candidate under a
/// coverage-weighted objective: the ranking proceeds over the measured plans
/// and the selection explanation names the plan it left out. Without that
/// sentence a populated selection reads as proof that every requested
/// provider was measured — exactly the outage that biases a ranking toward
/// whoever succeeded (the contract's §Selection states this exclusion).
#[test]
fn a_plan_that_never_acquired_is_not_a_candidate_and_the_selection_names_it() {
    let corpus = internal_corpus();
    let provider = provider_origin(recon_exa_body());
    let base_url = provider.url();
    let corpus_root = corpus.path().to_path_buf();
    let directory = tempfile::tempdir().expect("tempdir");

    let mut suite = suite(PolicyMode::Observe, vec![]);
    suite.providers = vec!["exa".into(), "internal".into()];
    suite.job.objective = Objective::Weighted {
        quality: 0.0,
        coverage: 1.0,
        freshness: 0.0,
        cost: 0.0,
        latency: 0.0,
        policy_risk: 0.0,
    };
    suite.coverage_rubric = Some(CoverageRubric {
        name: "corpus-rubric".into(),
        version: "1".into(),
        items: vec![RubricItem {
            name: "review-cadence".into(),
            any_of: vec!["review cadence".into()],
        }],
    });

    let options = RunOptions {
        // The open-web plan never acquires; the corpus still reads.
        allow_external_acquisition: false,
        output: directory.path().join("latest"),
        suppliers: Box::new(move |provider| {
            Ok(match provider {
                "internal" => Box::new(commonmeasure_supply::InternalCorpusAdapter::new(
                    &corpus_root,
                )) as Box<dyn commonmeasure_supply::SupplyAdapter>,
                _ => Box::new(ExaAdapter::new(&base_url, "test-key")),
            })
        }),
        backend: None,
        replay: None,
        allowance: None,
        provenance_signing:
            commonmeasure_runtime::processor::provenance::SigningIdentity::Unconfigured,
    };
    let summary = execute(&suite, &options)
        .expect("the run should complete")
        .summary;

    let plans = summary["plans"].as_array().expect("plans");
    let web = plans.iter().find(|plan| plan["id"] == "exa-only").unwrap();
    assert_eq!(web["status"], "unavailable");
    assert!(
        web["evaluation"]["coverage"]["fraction"].is_null(),
        "an unassembled window has no fraction to rank"
    );

    let selection = &summary["selection"];
    assert_eq!(
        selection["plan_id"], "internal-only",
        "the one measured plan is ranked; a failed acquisition does not force abstention"
    );
    assert!(
        selection["method"]
            .as_str()
            .is_some_and(|method| method.contains("coverage")),
        "the method names the measured term: {}",
        selection["method"]
    );
    assert!(
        selection["explanation"]
            .as_str()
            .is_some_and(|explanation| explanation.contains("exa-only was not a candidate")),
        "the explanation must name the plan the ranking left out: {}",
        selection["explanation"]
    );
    assert_eq!(
        selection["unavailable_inputs"],
        Value::Array(vec![]),
        "exclusion is stated in prose; unavailable_inputs names measurements only"
    );
}

/// Every plan carries every evaluator's section, each naming its evaluator —
/// and for a suite that did not require a cited answer, declared no rubric
/// and no as-of date, each section says the plan is unevaluated or
/// unmeasured and why. A constant standing in for a measurement is still the
/// specific failure this asserts against: no verdict and no fraction may
/// exist where nothing was checkable.
#[test]
fn every_plan_carries_an_evaluation_record_that_states_what_was_checkable() {
    let harness = Harness::run(&suite(PolicyMode::Strict, vec![]), true);
    for plan in harness.summary["plans"].as_array().unwrap() {
        let grounding = &plan["evaluation"]["grounding"];
        assert_eq!(
            grounding["evaluator"]["name"], "grounding",
            "plan {} carries no attributable grounding record",
            plan["id"]
        );
        assert!(
            grounding["unevaluated"]
                .as_str()
                .is_some_and(|reason| reason.contains("did not require a cited answer")),
            "this suite did not require citations, so plan {} must be unevaluated with the \
             reason stated",
            plan["id"]
        );
        assert_eq!(
            grounding["citations"].as_array().map(Vec::len),
            Some(0),
            "nothing was checkable, so no verdict may exist for plan {}",
            plan["id"]
        );
        // The rankable measures follow the same discipline: this suite
        // declared no rubric and no as_of, so both say unmeasured and carry
        // no fraction — never a zero.
        for (name, section) in [
            ("coverage", &plan["evaluation"]["coverage"]),
            ("freshness", &plan["evaluation"]["freshness"]),
        ] {
            assert_eq!(
                section["evaluator"]["name"], name,
                "plan {} carries no attributable {name} record",
                plan["id"]
            );
            assert!(
                section["unmeasured"].as_str().is_some(),
                "no {name} basis was declared, so plan {} must say unmeasured",
                plan["id"]
            );
            assert!(
                section["fraction"].is_null(),
                "an unmeasured {name} must carry no fraction for plan {}, least of all zero",
                plan["id"]
            );
        }
    }
}

/// The live-path proof for supplier-declared dates, offline: a real adapter
/// parses a response whose results declare `publishedDate`, no replay
/// manifest is anywhere in sight, and the freshness record measures the
/// window from the supplier's claims — each part naming supplier-declared
/// provenance. Before freshness/0.2.0 a live acquisition had no date source
/// at all and this suite's freshness could only abstain.
#[test]
fn a_live_acquisition_with_supplier_declared_dates_measures_freshness() {
    let mut suite = suite(PolicyMode::Strict, vec![]);
    suite.as_of = Some(commonmeasure_runtime::AsOf {
        date: "2026-08-05".to_owned(),
        maximum_age_days: 90,
    });
    let body = serde_json::to_vec(&serde_json::json!({
        "results": [
            {"url": "https://dated.example/a", "title": "t", "text": "a recent briefing",
             "publishedDate": "2026-07-20T00:00:00.000Z"},
            {"url": "https://stale.example/b", "title": "t", "text": "an old bulletin",
             "publishedDate": "2025-01-01T00:00:00.000Z"}
        ]
    }))
    .unwrap();
    let harness = Harness::run_against(&suite, body, false);
    let plan = harness.plan("exa-only");

    assert_eq!(plan["verification_state"], "live-verified");
    let freshness = &plan["evaluation"]["freshness"];
    assert_eq!(freshness["unmeasured"], Value::Null);
    assert_eq!(freshness["fresh_count"], 1);
    assert_eq!(freshness["part_count"], 2);
    assert_eq!(freshness["fraction"], 0.5);
    for part in freshness["parts"].as_array().expect("parts") {
        assert!(
            part["date_provenance"]
                .as_str()
                .is_some_and(|provenance| provenance.contains("supplier-declared")),
            "a live part's date must name the supplier as its declarer: {part}"
        );
    }
    assert_eq!(freshness["parts"][0]["within_maximum_age"], true);
    assert_eq!(freshness["parts"][1]["within_maximum_age"], false);
}

/// The same live path over the recorded Exa bytes, which declare no date:
/// with no replay manifest to lend a capture date, every part is undated and
/// the plan is unmeasured naming them — and no undated part ever reads as
/// fresh or stale, in the fraction or in its own row.
#[test]
fn a_live_acquisition_without_supplier_dates_stays_unmeasured_never_stale() {
    let mut suite = suite(PolicyMode::Strict, vec![]);
    suite.as_of = Some(commonmeasure_runtime::AsOf {
        date: "2026-08-05".to_owned(),
        maximum_age_days: 90,
    });
    let harness = Harness::run(&suite, false);
    let plan = harness.plan("exa-only");

    let freshness = &plan["evaluation"]["freshness"];
    assert!(
        freshness["unmeasured"]
            .as_str()
            .is_some_and(|reason| reason.contains("no parseable declared date")),
        "an undated live window must say why it is unmeasured: {}",
        freshness["unmeasured"]
    );
    assert_eq!(freshness["fraction"], Value::Null);
    assert_eq!(freshness["fresh_count"], Value::Null);
    for part in freshness["parts"].as_array().expect("parts") {
        assert_eq!(part["declared_date"], Value::Null);
        assert_eq!(
            part["within_maximum_age"],
            Value::Null,
            "an undated part is unknown — never fresh, never stale: {part}"
        );
        assert_eq!(part["age_days"], Value::Null);
    }
}

/// A plan that ran no inference has no inference latency to add, which
/// is not the same as adding zero. The published total must be the acquisition
/// leg alone, and the inference record must be absent rather than a zero-timed
/// one.
#[test]
fn a_plan_without_inference_reports_the_acquisition_leg_alone() {
    let harness = Harness::run(&suite(PolicyMode::Strict, vec![]), false);
    let plan = harness.plan("exa-only");

    assert_eq!(plan["status"], "unavailable");
    assert!(plan["inference"].is_null(), "no inference leg ran");
    assert_eq!(
        plan["latency_ms"], plan["acquisition"]["latency_ms"],
        "the total must be the one leg that ran, not that leg plus a fictional zero"
    );
}

/// The published summary attests to a log that is already closed, so its record
/// count includes the run's own terminal record.
#[test]
fn the_summary_attests_to_a_finished_evidence_log() {
    let harness = Harness::run(&suite(PolicyMode::Strict, vec![]), true);

    assert_eq!(harness.summary["run"]["evidence_complete"], true);
    let digest = harness.summary["evidence_log"]["sha256"]
        .as_str()
        .expect("the summary carries the log's digest");

    let bytes = std::fs::read(harness.published().join("evidence.ndjson")).expect("read log");
    assert_eq!(
        digest,
        format!(
            "sha256:{:x}",
            <sha2::Sha256 as sha2::Digest>::digest(&bytes)
        ),
        "the digest must match the log the run actually published"
    );
    assert_eq!(
        harness.summary["evidence_log"]["records"].as_u64().unwrap(),
        bytes.iter().filter(|b| **b == b'\n').count() as u64
    );

    let records =
        commonmeasure_runtime::EvidenceLog::read(&harness.published().join("evidence.ndjson"))
            .expect("read back");
    assert_eq!(
        records.last().unwrap()["event"],
        "run_completed",
        "the log was closed before the summary described it"
    );

    // `started_at` is when the run started, not when the summary was built:
    // it cannot postdate the log's first record.
    let started_at = chrono::DateTime::parse_from_rfc3339(
        harness.summary["run"]["started_at"].as_str().unwrap(),
    )
    .expect("started_at is RFC 3339");
    let first_record = chrono::DateTime::parse_from_rfc3339(
        records.first().unwrap()["timestamp"].as_str().unwrap(),
    )
    .expect("the log's timestamps are RFC 3339");
    assert!(
        started_at <= first_record,
        "a start time later than the run's first record was read after the run"
    );
}
