//! The grounding evaluator, exercised end to end: recorded supply through the
//! real adapters and policy, real inference transport against a loopback
//! gateway, and the evaluation record checked against what actually entered
//! the window.
//!
//! The gateway here is scripted, which is the one substitution `AGENTS.md`
//! allows: a local process stands in for the uncontrolled network, never for
//! this runtime's own path. Each scripted model reads the SUPPLIED CONTEXT it
//! is genuinely sent and answers from it — a faithful model cites what it
//! received, a misattributing model quotes words no source contains, a
//! stray model cites a URL outside the window — so every verdict the
//! evaluator reaches is checked against real window content, not fixtures.
//!
//! Two of the models below read the raw request — the system prompt and the
//! user message as sent, with no helper pre-parsing the window for them.
//! `context_blocks` performs, for a scripted model, exactly the
//! disambiguation the live model failed to perform on 4 August 2026
//! (the SOURCE prefix collision, `tests/fixtures/README.md`), so the tests
//! that close that seam must not
//! touch it.

use std::path::PathBuf;

use commonmeasure_http::{Response, Server, ServerHandle};
use commonmeasure_inference::TensorZeroBackend;
use commonmeasure_runtime::{ReplaySupply, Suite, execute};
use commonmeasure_types::{Constraint, ContextJob, Money, Objective, PolicyMode};
use serde_json::Value;
use uuid::Uuid;

fn repo_root() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/commonmeasure-runtime sits two levels below the repository root")
        .to_path_buf()
}

fn recon() -> PathBuf {
    repo_root().join("demo/recon")
}

/// One SOURCE block as the model received it: the URL and the exact text.
fn context_blocks(user_message: &str) -> Vec<(String, String)> {
    let Some((_, context)) = user_message.split_once("SUPPLIED CONTEXT\n") else {
        return Vec::new();
    };
    context
        .split("\n\nSOURCE ")
        .filter_map(|block| {
            let block = block.strip_prefix("SOURCE ").unwrap_or(block);
            let (header, text) = block.split_once('\n')?;
            let url = header.split(" [").next()?.trim();
            if url.is_empty() {
                return None;
            }
            Some((url.to_owned(), text.to_owned()))
        })
        .collect()
}

/// A loopback gateway whose "model" reads the raw request itself: the system
/// prompt and the user message exactly as this runtime sent them, nothing
/// pre-parsed on its behalf.
fn raw_scripted_gateway<F>(answer: F) -> ServerHandle
where
    F: Fn(&str, &str) -> String + Send + Sync + 'static,
{
    Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |request| {
            let body: Value = serde_json::from_slice(&request.body).expect("a JSON body");
            let system = body["messages"][0]["content"].as_str().unwrap_or_default();
            let user = body["messages"][1]["content"].as_str().unwrap_or_default();
            let content = answer(system, user);
            Response::json(
                200,
                &serde_json::json!({
                    "id": "tz-evaluation",
                    "model": "local-test-model",
                    "choices": [{"message": {"content": content}}],
                    "usage": {"prompt_tokens": 40, "completion_tokens": 9}
                })
                .to_string(),
            )
        })
        .expect("spawn")
}

/// The convenient scripting surface: the window pre-parsed into (url, text)
/// blocks. Fine for models whose behaviour under test is quoting; the seam
/// tests above use the raw gateway instead, because this helper performs the
/// model's disambiguation for it.
fn scripted_gateway(answer: fn(&[(String, String)]) -> String) -> ServerHandle {
    raw_scripted_gateway(move |_system, user| answer(&context_blocks(user)))
}

/// The first `words` words of `text`, exactly as they appear.
fn leading_words(text: &str, words: usize) -> String {
    text.split_whitespace()
        .take(words)
        .collect::<Vec<_>>()
        .join(" ")
}

fn suite(require_cited_answer: bool) -> Suite {
    Suite {
        suite_version: "evaluation-test/v1".into(),
        label: "Grounding evaluation path".into(),
        job: ContextJob {
            id: Uuid::new_v4(),
            kind: "research.answer".into(),
            prompt: "What do the supplied sources establish?".into(),
            policy_mode: PolicyMode::Strict,
            objective: Objective::MinimiseLatency,
            constraints: vec![Constraint::MaximumAcquisitionCost {
                amount: Money::from_decimal_str("USD", "0.05").expect("a valid cap"),
            }],
            evidence_requirements: vec![],
        },
        model_plan: commonmeasure_types::ModelPlan {
            name: "pinned".into(),
            version: "1".into(),
            model: "local-test-model".into(),
        },
        result_limit: 2,
        providers: vec!["exa".into()],
        require_cited_answer,
        coverage_rubric: None,
        as_of: None,
        governance: None,
        fetch_target: None,
        fidelity_judge: false,
        output_provenance: None,
    }
}

fn run(suite: &Suite, answer: fn(&[(String, String)]) -> String) -> Value {
    run_with(suite, scripted_gateway(answer))
}

fn run_with(suite: &Suite, gateway: ServerHandle) -> Value {
    let supply = ReplaySupply::start(&recon(), &suite.providers)
        .expect("the committed manifest covers every suite provider");
    let directory = tempfile::tempdir().expect("tempdir");
    let mut options = supply.run_options(directory.path().join("run"));
    options.backend = Some(Box::new(
        TensorZeroBackend::new(gateway.url()).expect("a loopback gateway endpoint"),
    ));
    execute(suite, &options)
        .expect("the replay run should complete")
        .summary
}

/// The published `manifest.json` of a run over `suite`: the hash beside the
/// sealed object, read before the temporary directory goes.
fn sealed_manifest(suite: &Suite) -> Value {
    let supply = ReplaySupply::start(&recon(), &suite.providers)
        .expect("the committed manifest covers every suite provider");
    let gateway = scripted_gateway(|_| "answer".to_owned());
    let directory = tempfile::tempdir().expect("tempdir");
    let mut options = supply.run_options(directory.path().join("run"));
    options.backend = Some(Box::new(
        TensorZeroBackend::new(gateway.url()).expect("a loopback gateway endpoint"),
    ));
    let report = execute(suite, &options).expect("the replay run should complete");
    let bytes = std::fs::read(report.output.join("manifest.json")).expect("manifest.json");
    serde_json::from_slice(&bytes).expect("manifest.json should be valid JSON")
}

fn exa_evaluation(summary: &Value) -> &Value {
    let plan = summary["plans"]
        .as_array()
        .expect("plans")
        .iter()
        .find(|plan| plan["id"] == "exa-only")
        .expect("the exa plan");
    assert_eq!(plan["status"], "completed", "inference must have run");
    &plan["evaluation"]["grounding"]
}

/// A model that quotes what it was genuinely given is supported, and the
/// recorded span excises from the window text to the quoted words.
#[test]
fn a_faithful_citation_is_supported_and_its_span_excises_to_the_quote() {
    let summary = run(&suite(true), |blocks| {
        let (url, text) = &blocks[0];
        format!(
            "The sources establish this.\nCITATION: {url} :: \"{}\"",
            leading_words(text, 6)
        )
    });
    let evaluation = exa_evaluation(&summary);

    assert_eq!(evaluation["unevaluated"], Value::Null);
    assert_eq!(evaluation["verdict_counts"]["supported"], 1);
    let citation = &evaluation["citations"][0];
    assert_eq!(citation["verdict"], "supported");
    assert!(
        citation["matched"]["content_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256:")),
        "a supported citation names the window part it matched"
    );
    assert!(
        citation["matched"]["end"].as_u64() > citation["matched"]["start"].as_u64(),
        "the recorded span must be non-empty"
    );
}

/// The same run shape with only the scripted answer changed: a quote no
/// source contains is contradicted, never folded into supported — the
/// acceptance sentence, pinned (changing the answer changes the evaluation
/// record for an explainable reason).
#[test]
fn a_quote_no_window_source_contains_is_contradicted() {
    let summary = run(&suite(true), |blocks| {
        let (url, _) = &blocks[0];
        format!(
            "The sources establish this.\n\
             CITATION: {url} :: \"words the recording has never contained at all\""
        )
    });
    let evaluation = exa_evaluation(&summary);

    assert_eq!(evaluation["verdict_counts"]["contradicted"], 1);
    assert_eq!(evaluation["verdict_counts"]["supported"], 0);
    let citation = &evaluation["citations"][0];
    assert_eq!(citation["matched"], Value::Null);
    assert!(
        citation["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("does not contain this word sequence"))
    );
}

/// A model writes a URL the way markdown writes one. The brackets are
/// decoration, not the source's identity: a citation of a URL this run
/// genuinely admitted must never publish as one the window never saw, least
/// of all in a record whose own `inputs` names that URL.
#[test]
fn an_autolinked_url_is_resolved_against_the_window_not_read_literally() {
    let summary = run(&suite(true), |blocks| {
        let (url, text) = &blocks[0];
        format!(
            "The sources establish this.\nCITATION: <{url}> :: \"{}\"",
            leading_words(text, 6)
        )
    });
    let evaluation = exa_evaluation(&summary);

    assert_eq!(evaluation["verdict_counts"]["supported"], 1);
    assert_eq!(evaluation["verdict_counts"]["uncovered"], 0);
    let cited = evaluation["citations"][0]["url"].as_str().expect("a URL");
    assert!(
        evaluation["inputs"]
            .as_array()
            .expect("inputs")
            .iter()
            .any(|input| input["reference"] == cited),
        "the record must resolve the citation to a part it lists as window input"
    );
}

/// A quote is cut out of running prose and the cut lands where the quoter
/// chose. A trailing stop the source does not carry is a difference in
/// punctuation, not a misattribution, and must not be published as one.
#[test]
fn a_quote_differing_only_in_trailing_punctuation_is_supported() {
    let summary = run(&suite(true), |blocks| {
        let (url, text) = &blocks[0];
        format!(
            "The sources establish this.\nCITATION: {url} :: \"{}.\"",
            leading_words(text, 6)
        )
    });
    let evaluation = exa_evaluation(&summary);

    assert_eq!(evaluation["verdict_counts"]["supported"], 1);
    assert_eq!(evaluation["verdict_counts"]["contradicted"], 0);
}

/// A gateway can return an empty completion. Scoring it published a plan
/// completed with four zero counts and a record asserting the answer declared
/// no citations — a measured zero standing in for nothing produced.
#[test]
fn an_empty_answer_is_unevaluated_rather_than_scored_at_zero() {
    let summary = run(&suite(true), |_| String::new());
    let evaluation = exa_evaluation(&summary);

    assert!(
        evaluation["unevaluated"]
            .as_str()
            .is_some_and(|reason| reason.contains("the answer is empty")),
        "an empty answer is uncheckable, not checked and clean"
    );
    assert_eq!(evaluation["citations"].as_array().map(Vec::len), Some(0));
}

/// A citation of a URL that never entered the window is uncovered: the
/// supplied context cannot back it, whatever the quote says.
#[test]
fn a_citation_outside_the_window_is_uncovered() {
    let summary = run(&suite(true), |_| {
        "The sources establish this.\n\
         CITATION: https://not-in-the-window.example/page :: \"any words at all\""
            .to_owned()
    });
    let evaluation = exa_evaluation(&summary);

    assert_eq!(evaluation["verdict_counts"]["uncovered"], 1);
    assert_eq!(evaluation["citations"][0]["verdict"], "uncovered");
}

/// A suite that did not require a cited answer is unevaluated with the
/// reason stated — the evaluator never grades an answer against a format it
/// was not asked to produce.
#[test]
fn a_suite_without_the_directive_is_unevaluated_with_the_reason() {
    let summary = run(&suite(false), |_| "An answer with no citations.".to_owned());
    let evaluation = exa_evaluation(&summary);

    assert!(
        evaluation["unevaluated"]
            .as_str()
            .is_some_and(|reason| reason.contains("did not require a cited answer"))
    );
    assert_eq!(evaluation["evaluator"]["name"], "grounding");
}

/// The first model to cross the prompt/parser seam the way the live model
/// did on 4 August 2026: it copies the raw `SOURCE <url> [<hash>]` header
/// line whole into its citation, label and hash included, read straight off
/// the user message — no helper cleans the header for it. A URL copied from
/// the window's own labelling must resolve to the admitted source, never
/// publish as one outside the window.
#[test]
fn a_citation_copying_the_raw_window_header_verbatim_is_supported() {
    let summary = run_with(
        &suite(true),
        raw_scripted_gateway(|_system, user| {
            let Some((_, context)) = user.split_once("SUPPLIED CONTEXT\n") else {
                return "The supplied context cannot establish this.".to_owned();
            };
            let mut lines = context.lines();
            let Some(header) = lines.by_ref().find(|line| line.starts_with("SOURCE ")) else {
                return "The supplied context cannot establish this.".to_owned();
            };
            // The quote is the opening words of the line under the header —
            // read from the window as sent, like the header itself.
            let quote = lines
                .next()
                .map(|line| {
                    line.split_whitespace()
                        .take(4)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            format!("The sources establish this.\nCITATION: {header} :: \"{quote}\"")
        }),
    );
    let evaluation = exa_evaluation(&summary);

    assert_eq!(
        evaluation["verdict_counts"]["uncovered"], 0,
        "a URL copied from the window's own header must never publish as outside the window"
    );
    assert_eq!(evaluation["verdict_counts"]["supported"], 1);
    let citation = &evaluation["citations"][0];
    assert_eq!(citation["verdict"], "supported");
    // The record names the bare URL its own `inputs` names, not the labelled
    // header it was written as.
    let cited = citation["url"].as_str().expect("a URL");
    assert!(
        !cited.contains("SOURCE") && !cited.contains('['),
        "the window's labelling leaked into the recorded URL: {cited}"
    );
    assert!(
        evaluation["inputs"]
            .as_array()
            .expect("inputs")
            .iter()
            .any(|input| input["reference"] == cited),
        "the resolved URL must be a window input"
    );
}

/// The second model does what the live model did on the no-context plan: it
/// repeats the directive's own template line, read out of the system prompt
/// it was genuinely sent — the first test anywhere to feed `messages[0]` to
/// something that answers. A template echo cites nothing, so no verdict
/// about the window belongs on it: `unavailable` with the placeholder named,
/// never `uncovered`, which asserts a fact about the window.
#[test]
fn a_model_echoing_the_directives_template_is_unavailable_not_uncovered() {
    let summary = run_with(
        &suite(true),
        raw_scripted_gateway(|system, _user| {
            let template = system
                .lines()
                .find(|line| line.trim_start().starts_with("CITATION:"))
                .expect("a cited suite's system prompt teaches the citation form");
            format!("The supplied context cannot establish this.\n{template}")
        }),
    );
    let evaluation = exa_evaluation(&summary);

    assert_eq!(
        evaluation["verdict_counts"]["uncovered"], 0,
        "a template echo is not a fact about the window"
    );
    assert_eq!(evaluation["verdict_counts"]["unavailable"], 1);
    let citation = &evaluation["citations"][0];
    assert_eq!(citation["verdict"], "unavailable");

    // The reason names the placeholder itself, as the directive spells it.
    let placeholder = commonmeasure_runtime::evaluate::CITATION_DIRECTIVE
        .lines()
        .find(|line| line.trim_start().starts_with("CITATION:"))
        .and_then(|line| line.split_once("::"))
        .map(|(url, _)| url.trim_start().trim_start_matches("CITATION:").trim())
        .expect("the directive shows its placeholder");
    let reason = citation["reason"].as_str().expect("a reason");
    assert!(
        reason.contains(placeholder),
        "the reason must name the placeholder {placeholder:?}: {reason}"
    );
}

/// The directive is part of experiment identity: the manifest seals the
/// composed system prompt and the installed evaluator, so a citing suite and
/// a non-citing suite are different experiments by hash.
#[test]
fn the_directive_and_the_evaluator_are_sealed_in_the_manifest() {
    let plain_suite = suite(false);
    let mut cited_suite = plain_suite.clone();
    cited_suite.require_cited_answer = true;

    let plain = sealed_manifest(&plain_suite);
    let cited = sealed_manifest(&cited_suite);

    // One cloned suite, so the two runs share a job id and everything else the
    // manifest seals: the directive is the only thing left that can move the
    // hash.
    assert_eq!(
        plain["manifest"]["job"]["id"], cited["manifest"]["job"]["id"],
        "the two suites must differ in the directive alone"
    );
    assert_ne!(
        plain["manifest"]["system_prompt"], cited["manifest"]["system_prompt"],
        "the citation directive must reach the sealed system prompt"
    );
    assert_ne!(
        plain["hash"], cited["hash"],
        "a suite that requires citations must not claim to be the same experiment"
    );

    // What will judge the answers is part of what the comparison is, so the
    // manifest names every installed evaluator by identity.
    let evaluators: Vec<&str> = cited["manifest"]["evaluators"]
        .as_array()
        .expect("the manifest seals the installed evaluator identities")
        .iter()
        .map(|evaluator| evaluator["name"].as_str().expect("an evaluator name"))
        .collect();
    assert_eq!(
        evaluators,
        ["grounding", "coverage", "freshness"],
        "the manifest must name the evaluators installed for this run"
    );
    for evaluator in cited["manifest"]["evaluators"]
        .as_array()
        .expect("the installed evaluators")
    {
        assert!(
            evaluator["version"].is_string() && evaluator["configuration_digest"].is_string(),
            "an evaluator identity is its name, its version and its configuration digest"
        );
    }
}

/// The governance block is part of experiment identity, and so is every rule
/// in it: an ungoverned suite, a governed suite and the same governed suite
/// with one rule's effective date flipped are three different experiments by
/// hash. This is the traceability the revocation demonstration rests on —
/// the manifest names nothing inside the internal corpus, so the rules must
/// be sealed here or a flipped rule would move no hash at all.
#[test]
fn the_governance_block_and_each_rule_are_sealed_in_the_manifest() {
    let governance = |effective_date: &str| {
        serde_json::from_value::<commonmeasure_runtime::governance::Governance>(serde_json::json!({
            "name": "fictive-support-rules",
            "version": "1",
            "entitlement_tiers": ["standard", "premier"],
            "granted_entitlement": "standard",
            "rules": [{
                "edition": "orchestrator",
                "version_range": "4.x",
                "integration_path": "flux-connector",
                "support_status": "deprecated",
                "effective_date": effective_date
            }]
        }))
        .expect("a well-formed governance block")
    };
    let as_of = || {
        serde_json::from_value::<commonmeasure_runtime::freshness::AsOf>(serde_json::json!({
            "date": "2026-08-06",
            "maximum_age_days": 365
        }))
        .expect("a well-formed as_of")
    };
    // One cloned suite throughout, so a fresh job id cannot stand in for the
    // difference the assertions are about.
    let ungoverned_suite = suite(false);
    let mut governed_suite = ungoverned_suite.clone();
    governed_suite.as_of = Some(as_of());
    governed_suite.governance = Some(governance("2026-03-01"));
    let mut revoked_suite = governed_suite.clone();
    revoked_suite.governance = Some(governance("2026-09-01"));

    let ungoverned = sealed_manifest(&ungoverned_suite);
    let governed = sealed_manifest(&governed_suite);
    let revoked = sealed_manifest(&revoked_suite);

    assert_eq!(
        ungoverned["manifest"]["job"]["id"], revoked["manifest"]["job"]["id"],
        "the three suites must differ in the governance block alone"
    );
    assert_ne!(
        ungoverned["hash"], governed["hash"],
        "declaring governance must not claim to be the same experiment"
    );
    assert_ne!(
        governed["hash"], revoked["hash"],
        "flipping one rule's effective date must move the manifest hash"
    );
}
