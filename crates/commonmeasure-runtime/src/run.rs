//! One run, from a declared suite to a published, inspectable directory.
//!
//! The path: ContextJob, deterministic eligibility and policy, a real provider
//! adapter, a real transport boundary, the same parser a live call uses,
//! admitted input, a real configured inference backend, append-only evidence
//! and a sealed manifest.
//!
//! Where a real dependency is absent the run says so and stops there. A missing
//! credential, an unconfigured gateway or a policy refusal each produce an
//! explicit unavailable or refused plan with a gap naming what is missing.
//! There is no mode in which this runtime answers anyway.

use commonmeasure_types::canonical::sha256_digest;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use commonmeasure_inference::{
    ContextPart, InferenceBackend, InferenceError, InferenceRequest, backend_from_environment,
};
use commonmeasure_supply::{
    Acquisition, SupplyAdapter, SupplyError, SupplyQuote, supplier_from_environment,
};
use commonmeasure_types::canonical::canonical_digest;
use commonmeasure_types::{
    AssuranceBasis, ContextJob, Decision, DecisionRecord, Gap, GapReason, ModelPlan,
    ProviderCapability, SupplyPlan, SupplyStep,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::allowance::Reservation;
use crate::coverage::{self, CoverageRubric};
use crate::evaluate;
use crate::evidence::{EvidenceLog, RunDirectory};
use crate::freshness::{self, AsOf, DeclaredDate};
use crate::governance::Governance;
use crate::policy::{self, Ruling, TOKEN_BASIS, fit_context_budget};
use crate::processor;
use crate::processor::provenance::{self, Grade, OutputProvenance, SigningIdentity};
use crate::selection::{self, Candidate, MeasuredCoverage, Selection};

/// The run-output contract version. Any change to the shape below changes this.
/// v2: plans carry `processors` (invocation records), and the manifest names
/// the installed processors whose configuration shaped the run. A plan's step
/// may use the `query` capability, its acquisition then records
/// `http_status: null` (no HTTP happened), and `coverage_gap` joins the gap
/// vocabulary for bounded corpora.
/// v3: recorded replay is a run mode. `run.mode` gains `replay` and the run
/// then carries `replay_manifest`; a replayed plan claims `replay-tested`,
/// its acquisition carries a `replay` record binding the sealed response to
/// its recorded source, and the run directory publishes `replay.json`.
/// v4: a plan's `evaluation` is a record of evaluator sections — `grounding`
/// (the v3 record, unchanged in shape), `coverage` and `freshness` — because
/// three evaluators now run per plan and each section carries its own
/// identity, basis and unmeasured reason. Suites may opt into
/// `coverage_rubric` and `as_of`, both sealed by the manifest. Suites may
/// also opt into `governance` — a support-status rule set and entitlement
/// grant evaluated at admission — sealed by the manifest
/// (contextops-manifest/v4); the record shapes above are unchanged, the
/// governance verdicts riding the existing processor-invocation records.
/// v5: quote-then-buy is a run shape. A plan whose provider declares the
/// `quote` capability prices the acquisition first: `acquisition.quote`
/// carries the sealed quote legs (their refs and hashes), the quoted charge,
/// the provider's billing status and verbatim billing note, and the purchase
/// decision — entered into evidence *before* any purchase. A declined quote
/// is a `refused` plan carrying the quote record; the purchase decision is
/// the plan's cost ruling (the post-hoc cap check does not run again over
/// the receipt, which would be a second account of the same decision).
/// v6: skill supply is a run shape. A plan may name a `skill:<name>`
/// provider, whose step uses the `invoke` capability; its acquisition carries
/// `invocation` — the skill's declared identity and digests, the resolved
/// interpreter, cwd and verbatim argv with each element's provenance, the
/// environment policy, the exit status, the stream sizes and hashes, and the
/// duration against the declared timeout. A non-zero exit is a recorded
/// result, not a run error. The `--live` gate widens from "money leaves the
/// operator" to "money leaves the operator or code executes", so an
/// invocation happens only when asked for.
/// v7: the principal's cumulative allowance is consulted at every quoted
/// purchase. `acquisition.quote.allowance` records the consultation — the
/// principal, each declared period's arithmetic at the moment of the check,
/// the decision, and after settlement the reconciliation against the receipt
/// with its ledger note — or states explicitly that no allowance context was
/// held (a run without principal policy, or replay, where no money moves).
/// The allowance is the second pre-spend gate beside the job's own cost cap
/// and the two stay separate records: one bounds a purchase, the other a
/// period of them.
pub const SCHEMA_VERSION: &str = "contextops-run/v7";

/// The system instruction sent with every plan in a comparison. Fixed, because
/// varying it would vary the experiment.
const SYSTEM_PROMPT: &str = "Answer only from the supplied context. State plainly what the supplied context cannot \
     establish. Do not use knowledge that is not in the supplied context.";

/// A versioned suite: one job, one model plan, one fixed acquisition
/// configuration, and the providers to compare.
///
/// Unknown fields are load errors. A suite is the experiment's declaration,
/// and a misspelled opt-in — `governence`, `coverage_rubrik` — would
/// otherwise run a different experiment from the one the operator wrote,
/// silently.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Suite {
    pub suite_version: String,
    pub label: String,
    pub job: ContextJob,
    pub model_plan: ModelPlan,
    /// Maximum results requested from every provider. Fixed across the
    /// comparison so no provider is asked for more evidence than another.
    pub result_limit: u32,
    /// Providers to compare, by adapter name.
    pub providers: Vec<String>,
    /// Ask the model to end its answer with `CITATION:` lines in the
    /// grounding evaluator's format (`crate::evaluate::CITATION_DIRECTIVE`).
    /// Off by default: the directive extends the sealed system prompt, so a
    /// suite that sets it is a different experiment from one that does not.
    #[serde(default)]
    pub require_cited_answer: bool,
    /// Predeclared items a complete window must draw on
    /// (`crate::coverage`). Opt-in like `require_cited_answer`, and sealed by
    /// the manifest: a suite that declares a rubric is a different experiment
    /// by hash. Without one, every coverage record says unmeasured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage_rubric: Option<CoverageRubric>,
    /// The reference date and staleness horizon freshness is measured
    /// against (`crate::freshness`). Suite input, never a clock, so replay
    /// determinism holds. Without one, every freshness record says
    /// unmeasured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of: Option<AsOf>,
    /// The support-status rule set and entitlement grant this run admits
    /// sources under (`crate::governance`). Opt-in like the rubric, and
    /// sealed by the manifest for the same reason: a suite that declares
    /// governance — or flips one rule — is a different experiment by hash.
    /// Without one, no governance processor runs and no invocation is
    /// recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub governance: Option<Governance>,
    /// A single URL to retrieve rather than a web to search. When present, the
    /// run is a fetch comparison: each provider that declares the `fetch`
    /// capability retrieves this exact URL (`crate` dispatches `fetch`, not
    /// `search`), and the job prompt and result limit do not govern the
    /// acquisition. Sealed by the manifest like every other opt-in, because a
    /// suite that names a fetch target is a different experiment from one that
    /// searches. Without it the run searches as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetch_target: Option<String>,
    /// Ask the fidelity judge (`crate::processor::judge`) for a verdict per
    /// claim of every answer, through the configured gateway under this
    /// suite's model plan. Off by default: the judge is a second model
    /// exchange per plan, and a suite that declares it is a different
    /// experiment, so the manifest seals it when set. The deterministic
    /// fidelity verifier runs on every answer regardless.
    #[serde(default)]
    pub fidelity_judge: bool,
    /// The operator's declaration for the run's own outputs: what may be done
    /// with them (`crate::processor::provenance`). Opt-in like governance and
    /// sealed by the manifest for the same reason. When declared, every
    /// answered plan is labelled with a C2PA manifest built from its source
    /// record, signed with the operator's certificate; without one, no
    /// provenance processor runs and no invocation is recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_provenance: Option<OutputProvenance>,
}

impl Suite {
    /// The system message actually sent — and sealed in the manifest: the
    /// fixed prompt, plus the citation directive when this suite requires a
    /// cited answer.
    fn system_prompt(&self) -> String {
        if self.require_cited_answer {
            format!("{SYSTEM_PROMPT}{}", crate::evaluate::CITATION_DIRECTIVE)
        } else {
            SYSTEM_PROMPT.to_owned()
        }
    }
}

#[derive(Debug)]
pub enum RunError {
    Suite(String),
    Io(std::io::Error),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Suite(detail) => write!(f, "{detail}"),
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for RunError {}

impl From<std::io::Error> for RunError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Resolves an adapter for a provider name.
///
/// The runtime takes this rather than reading the environment itself, so a test
/// can run the whole path against real adapters aimed at a loopback origin
/// serving recorded provider bytes. What is substituted is the origin, never
/// the adapter, the transport, the parser or the policy. This is the seam
/// recorded replay enters through (`docs/contracts/run-output.md` §Replay
/// binding).
///
/// The resolver decides nothing about the labels a run publishes. The run's
/// mode and a completed plan's verification state follow the caller's wiring —
/// [`RunOptions::replay`] present means mode `replay` and `replay-tested`,
/// absent with `allow_external_acquisition` set means mode `live` and
/// `live-verified` — and nothing observes where the resolved adapter's
/// requests actually went. A caller that aims real adapters at a loopback
/// origin without passing a [`crate::replay::ReplayContext`] therefore
/// publishes `live-verified`, which is what the end-to-end tests do.
pub type SupplyResolver =
    Box<dyn Fn(&str) -> Result<Box<dyn SupplyAdapter>, SupplyError> + Send + Sync>;

/// Everything a run needs from outside itself.
pub struct RunOptions {
    /// Whether external provider calls are authorised. Off by default: an
    /// acquisition can cost money, so it happens only when asked for.
    pub allow_external_acquisition: bool,
    pub output: PathBuf,
    pub suppliers: SupplyResolver,
    /// The configured inference gateway. `None` is a first-class state: the run
    /// records an explicit unavailable result rather than answering from
    /// somewhere else.
    pub backend: Option<Box<dyn InferenceBackend>>,
    /// Present when `suppliers` resolves to loopback origins serving verified
    /// recordings (`crate::replay`). It carries the provenance the run must
    /// then publish; acquisition proceeds without external authority because
    /// nothing in the path can reach a provider.
    pub replay: Option<crate::replay::ReplayContext>,
    /// The resolved principal's allowance, when the caller holds one
    /// (`commonmeasure-cli` resolves the operator home and principal; this crate
    /// resolves neither). `None` is recorded as an explicit absence on every
    /// quoted purchase, never treated as an unlimited allowance that happens
    /// to be missing.
    pub allowance: Option<crate::allowance::AllowanceContext>,
    /// The operator's signing certificate and key for output provenance
    /// labels, when the environment names them. `Unconfigured` is recorded
    /// as an explicit gap on every plan a suite asks to label.
    pub provenance_signing: SigningIdentity,
}

impl RunOptions {
    /// The ordinary configuration: adapters built from environment
    /// credentials, gateway from `COMMONMEASURE_INFERENCE_ENDPOINT`.
    pub fn from_environment(output: PathBuf, allow_external_acquisition: bool) -> Self {
        Self {
            allow_external_acquisition,
            output,
            suppliers: Box::new(supplier_from_environment),
            backend: backend_from_environment(),
            replay: None,
            allowance: None,
            provenance_signing: SigningIdentity::from_environment(),
        }
    }
}

pub struct RunReport {
    pub run_id: Uuid,
    pub manifest_hash: String,
    pub output: PathBuf,
    pub summary: Value,
}

/// Execute `suite` and publish the run.
pub fn execute(suite: &Suite, options: &RunOptions) -> Result<RunReport, RunError> {
    suite
        .job
        .validate()
        .map_err(|error| RunError::Suite(format!("invalid context job: {error:?}")))?;
    suite
        .model_plan
        .validate()
        .map_err(|error| RunError::Suite(format!("invalid model plan: {error:?}")))?;
    if suite.result_limit == 0 {
        return Err(RunError::Suite(
            "result_limit must be positive; a plan that asks for no results has nothing to compare"
                .to_owned(),
        ));
    }
    if suite.providers.is_empty() {
        return Err(RunError::Suite(
            "a suite must name at least one provider to compare".to_owned(),
        ));
    }
    if let Some(rubric) = &suite.coverage_rubric {
        rubric.validate().map_err(RunError::Suite)?;
    }
    if let Some(as_of) = &suite.as_of {
        as_of.validate().map_err(RunError::Suite)?;
    }
    if let Some(governance) = &suite.governance {
        governance.validate().map_err(RunError::Suite)?;
        // Rules take effect at dates and the reference date is suite input,
        // never a clock. A governed suite with no as_of could not know which
        // rules are in force, and guessing would govern by a date nobody
        // declared.
        if suite.as_of.is_none() {
            return Err(RunError::Suite(format!(
                "the suite declares governance {} but no as_of; rules take effect at dates, \
                 and without a declared reference date which rules are in force cannot be \
                 known",
                governance.identity()
            )));
        }
    }

    if let Some(provenance) = &suite.output_provenance {
        provenance.validate().map_err(RunError::Suite)?;
    }

    let supply_plans = supply_plans(suite)?;
    // A replay run answers every provider plan from a verified recording, so
    // a provider without one fails the run before anything executes. Failing
    // per-plan instead would publish a weaker run wearing the replay name.
    if let Some(replay) = &options.replay {
        for plan in &supply_plans {
            let provider = &plan.steps[0].provider.name;
            if !replay.recordings.contains_key(provider) {
                return Err(RunError::Suite(format!(
                    "replay provides no recording for provider {provider}; a replay run never \
                     substitutes live or local acquisition for a missing recording"
                )));
            }
        }
    }
    let backend = options.backend.as_deref();
    let run_id = Uuid::new_v4();
    // Taken before any plan executes: this is the timestamp the summary
    // publishes as `started_at`, and a start time read from the clock after
    // the run finished would be off by the whole run.
    let started_at = Utc::now();
    let manifest = manifest(suite, &supply_plans, backend, options);
    // Over the canonical form of the manifest, not over whatever bytes
    // serde_json happens to emit here, so the seal is a function of the
    // manifest alone and a reviewer recomputes it with any conforming
    // serialiser (`docs/contracts/run-output.md` §Manifest).
    let manifest_hash = canonical_digest(&manifest);

    let directory = RunDirectory::stage(&options.output)?;
    let mut log = EvidenceLog::create(&directory.path("evidence.ndjson")?)?;
    directory.write_json(
        "manifest.json",
        &json!({"hash": manifest_hash, "manifest": manifest}),
    )?;
    record(
        &mut log,
        "run_started",
        json!({"run_id": run_id, "manifest_hash": manifest_hash}),
    );

    let mut run = Execution {
        suite,
        options,
        backend,
        run_id,
        manifest_hash: &manifest_hash,
        directory: &directory,
        log: &mut log,
    };

    let mut plans = vec![run.baseline_plan()];
    for supply_plan in &supply_plans {
        plans.push(run.provider_plan(supply_plan)?);
    }

    // The inspectable comparison a replay run owes: every sealed response
    // bound to the recorded input it was served from, by hash equality a
    // reader can recheck against both files.
    if let Some(replay) = &options.replay {
        directory.write_json("replay.json", &replay_bindings(replay, &plans))?;
    }

    let (candidates, excluded) = candidates(&plans);
    let mut selection =
        selection::select(&suite.job.objective, &candidates, options.replay.is_some());
    name_excluded(&mut selection, &excluded);

    // The log is closed before the summary says anything about it. Reading
    // completeness first and appending the terminal record afterwards would
    // publish a claim that the very next write can falsify, and that write's
    // failure is deliberately not fatal — so nothing would ever correct it.
    let evidence = finalise_log(&mut log, run_id);

    let mut summary = json!({
        "schema_version": SCHEMA_VERSION,
        "suite_version": suite.suite_version,
        "run": {
            "id": run_id,
            "mode": acquisition_mode(options),
            "started_at": started_at.to_rfc3339_opts(SecondsFormat::Secs, true),
            "manifest_hash": manifest_hash,
            "fairness": FAIRNESS,
            "token_basis": TOKEN_BASIS,
            "evidence_complete": evidence.complete,
        },
        // The summary attests to the log rather than the other way round,
        // because the log is finished first and a finished log cannot carry
        // the hash of a document that does not exist yet.
        "evidence_log": {
            "records": evidence.records,
            "sha256": evidence.digest,
        },
        "job": job_projection(suite),
        "model_plan": {
            "requested_model": suite.model_plan.model,
            "gateway": backend.map(InferenceBackend::name),
            "endpoint": backend.map(InferenceBackend::endpoint),
            "configured": backend.is_some(),
        },
        "plans": plans,
        "selection": {
            "plan_id": selection.plan_id,
            "objective": suite.job.objective,
            "method": selection.method,
            "explanation": selection.explanation,
            "unavailable_inputs": selection.unavailable_inputs,
        },
    });
    if let Some(replay) = &options.replay {
        summary["run"]["replay_manifest"] = json!(replay.manifest_path);
    }
    directory.write_json("summary.json", &summary)?;
    let output = directory.publish()?;
    Ok(RunReport {
        run_id,
        manifest_hash,
        output,
        summary,
    })
}

/// One plan's admitted window as the evaluators receive it: the parts that
/// entered, and beside each one whatever declared a date for it — the
/// supplier's own claim from the envelope where the adapter mapped one,
/// else the replay recording's capture date when the plan was replayed. A
/// live acquisition whose supplier declared nothing has no date at all, and
/// the freshness record then says unmeasured rather than borrowing a clock.
struct Window<'a> {
    parts: &'a [ContextPart],
    /// One entry per part, in part order.
    dates: &'a [Option<DeclaredDate>],
}

/// One run in progress. Holding the collaborators together keeps every stage
/// below reading as the decision it makes rather than as a parameter list.
struct Execution<'a> {
    suite: &'a Suite,
    options: &'a RunOptions,
    backend: Option<&'a dyn InferenceBackend>,
    run_id: Uuid,
    manifest_hash: &'a str,
    directory: &'a RunDirectory,
    log: &'a mut EvidenceLog,
}

impl Execution<'_> {
    /// The controlled baseline: the same question with no external supply.
    ///
    /// It is the only honest answer to "did the context help?", so it runs
    /// whether or not external acquisition is authorised.
    fn baseline_plan(&mut self) -> Value {
        let mut plan = PlanUnderConstruction::new("no-context", "none", None);
        plan.add_gap(Gap::new(
            GapReason::EvidenceMissing,
            "The controlled baseline requested no external context, so nothing grounds its \
             answer.",
        ));
        let (status, inference) = match self.infer(&[]) {
            Ok(inference) => ("completed", inference),
            Err(gap) => {
                plan.add_gap(gap);
                ("unavailable", Value::Null)
            }
        };
        let latency = inference.get("latency_ms").and_then(Value::as_u64);
        let mut value = plan.finish(status, Value::Null, Vec::new(), latency, inference);
        value["eligibility"] =
            json!({"eligible": true, "reason": "The baseline uses no provider."});
        value["context_tokens_admitted"] = json!(0);
        // The baseline's window is assembled and deliberately empty: covering
        // no rubric item is its measured state, not an absence.
        self.record_plan(
            value,
            Some(&Window {
                parts: &[],
                dates: &[],
            }),
        )
    }

    fn provider_plan(&mut self, supply_plan: &SupplyPlan) -> Result<Value, RunError> {
        let step = &supply_plan.steps[0];
        let provider = step.provider.name.clone();
        let mut plan = PlanUnderConstruction::new(&supply_plan.name, &provider, Some(step));

        let eligibility = policy::provider_eligibility(&self.suite.job, &provider);
        plan.apply(&eligibility, self.run_id, &self.suite.job, None);
        if eligibility.is_refusal() {
            let value = plan.finish("refused", Value::Null, Vec::new(), None, Value::Null);
            return Ok(self.record_plan(value, None));
        }

        // The --live gate is about money leaving the operator or code
        // executing. An external provider call is billable and an invocation
        // runs a third party's program on this machine, so both happen only
        // when asked for. A corpus query is neither: the operator reads
        // supply they already own, no third party and no charge, so it runs
        // in the offline mode too, which is what lets the internal heatmap
        // exist without a single credential. A replay run is billable to
        // nobody and executes nothing: its resolver reaches only loopback
        // origins serving verified recordings, so it proceeds without
        // external authority.
        if !self.options.allow_external_acquisition
            && self.options.replay.is_none()
            && step.capability != ProviderCapability::Query
        {
            plan.add_gap(Gap::new(
                GapReason::EvidenceMissing,
                match step.capability {
                    ProviderCapability::Invoke => {
                        "This run was not authorised to execute anything, so the skill was not \
                         invoked and no process was spawned. Re-run with --live and a catalogue \
                         declaring it."
                    }
                    _ => {
                        "This run was not authorised to make external provider calls, so no \
                         acquisition was attempted. Re-run with --live and a configured \
                         credential."
                    }
                },
            ));
            let value = plan.finish("unavailable", Value::Null, Vec::new(), None, Value::Null);
            return Ok(self.record_plan(value, None));
        }

        // A quote-then-buy provider prices the acquisition first. The quote's
        // sealed legs, price and billing note enter the plan evidence before
        // the purchase decision, and a declined quote is a recorded decision
        // — a refused plan carrying the quote — never a silent skip.
        let mut quote: Option<(Value, u64, Ruling)> = None;
        // The dispatch gate's record and any reservation it holds, carried
        // out of the acquisition arm to be settled against the receipt (or
        // released on failure) and attached to the acquisition record.
        let mut dispatch_allowance: Option<(Value, Option<Reservation>)> = None;
        // Set where a provider's published page size is smaller than the
        // job's result limit, so the gap below can name both numbers.
        let mut capped_search: Option<u32> = None;
        let acquired = if step
            .provider
            .capabilities
            .contains(&ProviderCapability::Quote)
        {
            match self.quoted_purchase(&mut plan, supply_plan)? {
                QuoteStep::Failed(error) => Err(error),
                QuoteStep::NotPurchased {
                    record,
                    latency_ms,
                    status,
                } => {
                    let mut value = plan.finish(
                        status,
                        json!({ "quote": record }),
                        Vec::new(),
                        Some(latency_ms),
                        Value::Null,
                    );
                    // The quote legs completed and their exact bytes are
                    // sealed; that sealed exchange is what the state claims.
                    value["verification_state"] = json!(self.verification_state());
                    return Ok(self.record_plan(value, None));
                }
                QuoteStep::Purchased {
                    acquisition,
                    record,
                    latency_ms,
                    ruling,
                } => {
                    quote = Some((record, latency_ms, ruling));
                    *acquisition
                }
            }
        } else {
            match (self.options.suppliers)(&provider) {
                Err(error) => Err(error),
                Ok(supplier) => {
                    // The allowance gate stands where money can move: the
                    // remote search and fetch dispatches. A corpus query and
                    // a skill invocation have no supplier price by
                    // construction (`docs/contracts/provider.md`), so a
                    // monetary allowance has nothing to say there.
                    if matches!(
                        step.capability,
                        ProviderCapability::Search | ProviderCapability::Fetch
                    ) {
                        match self.options.allowance.as_ref() {
                            None => {
                                dispatch_allowance = Some((
                                    json!({
                                        "consulted": false,
                                        "reason": "the run holds no principal allowance \
                                                   context: it was started without principal \
                                                   policy, or in replay, where no money moves",
                                    }),
                                    None,
                                ));
                            }
                            Some(allowance) => {
                                let decision = allowance.pre_dispatch(
                                    supplier.published_price(step.capability).as_ref(),
                                    self.suite.job.policy_mode,
                                    Utc::now(),
                                    &format!("run {}", self.run_id),
                                );
                                if let Some(ruling) = &decision.ruling {
                                    plan.apply(ruling, self.run_id, &self.suite.job, None);
                                    if ruling.is_refusal() {
                                        let value = plan.finish(
                                            "refused",
                                            json!({ "allowance": decision.record }),
                                            Vec::new(),
                                            None,
                                            Value::Null,
                                        );
                                        return Ok(self.record_plan(value, None));
                                    }
                                }
                                dispatch_allowance = Some((decision.record, decision.reservation));
                            }
                        }
                    }
                    match step.capability {
                        ProviderCapability::Query => {
                            supplier.query(&self.suite.job.prompt, step.limit)
                        }
                        // One invocation, one result: the step's limit governs
                        // nothing here and is not passed, which is why the
                        // acquisition record says so in words rather than
                        // carrying a number that decided nothing.
                        ProviderCapability::Invoke => supplier.invoke(&self.suite.job.prompt),
                        // The URL is the request. A fetch plan only exists when the
                        // suite named a fetch_target, so it is present here; the
                        // prompt and limit do not govern the retrieval.
                        ProviderCapability::Fetch => match &self.suite.fetch_target {
                            Some(url) => supplier.fetch(url),
                            None => Err(SupplyError::CapabilityUnavailable {
                                provider: provider.clone(),
                                capability: ProviderCapability::Fetch,
                            }),
                        },
                        // Plans are built with search for everything else and
                        // validated against declared capabilities before execution.
                        // The job's allowed source hosts ride along so an adapter
                        // that can scope by domain fetches only what admission would
                        // keep; admission enforces the host policy regardless.
                        _ => {
                            // A provider that publishes a smaller page than
                            // the job asked for answers at its own size. The
                            // shortfall is recorded below rather than left
                            // silent, so a comparison cannot run at two sizes
                            // with nothing saying so.
                            capped_search = supplier
                                .maximum_search_results()
                                .filter(|cap| *cap < step.limit);
                            let include_hosts: Vec<&str> =
                                if self.suite.job.allows_provider_sources(&provider) {
                                    Vec::new()
                                } else {
                                    self.suite.job.allowed_source_hosts().collect()
                                };
                            supplier.search(&self.suite.job.prompt, step.limit, &include_hosts)
                        }
                    }
                }
            }
        };
        let acquisition = match acquired {
            Ok(acquisition) => acquisition,
            Err(error) => {
                plan.add_gap(supply_gap(&error));
                // A failure between a confirmed decision and its settlement
                // still carries the quote: the decision to buy was made and
                // recorded, and the evidence must show it.
                let (mut acquisition_value, latency) = match &quote {
                    Some((record, latency_ms, _)) => {
                        (json!({ "quote": record }), Some(*latency_ms))
                    }
                    None => (Value::Null, None),
                };
                // A held reservation outlives no failed dispatch: it is
                // released with the failure named, and the gate's record
                // still enters evidence — the consultation happened.
                if let Some((mut record, reservation)) = dispatch_allowance.take() {
                    if let (Some(allowance), Some(reservation)) =
                        (self.options.allowance.as_ref(), reservation.as_ref())
                        && let Err(release_error) = allowance.release_after_failure(
                            &mut record,
                            reservation,
                            &format!("the dispatch failed before any receipt ({error})"),
                            Utc::now(),
                        )
                    {
                        plan.add_gap(Gap::new(
                            GapReason::EvidenceMissing,
                            format!(
                                "The allowance reservation could not be released: \
                                 {release_error}"
                            ),
                        ));
                    }
                    acquisition_value = json!({ "allowance": record });
                }
                let value = plan.finish(
                    "unavailable",
                    acquisition_value,
                    Vec::new(),
                    latency,
                    Value::Null,
                );
                return Ok(self.record_plan(value, None));
            }
        };

        if let Some(cap) = capped_search {
            plan.add_gap(Gap::new(
                GapReason::CoverageGap,
                format!(
                    "This job asked {provider} for {} results and {provider} publishes a search \
                     page of {cap}, so {cap} were requested. A plan compared against one that \
                     ran at the job's own size ran at a different size.",
                    step.limit
                ),
            ));
        }

        // Only a bounded corpus can claim a coverage gap: it knows its own
        // extent, so results falling short of the request is a fact about the
        // supply, not about a search engine's mood. This is the record "where
        // did our internal content fail to cover the job" is built from.
        if step.capability == ProviderCapability::Query
            && acquisition.envelopes.len() < step.limit as usize
        {
            plan.add_gap(Gap::new(
                GapReason::CoverageGap,
                format!(
                    "The corpus held {} document(s) matching this job against the {} requested; \
                     the sealed query response names every document scanned.",
                    acquisition.envelopes.len(),
                    step.limit
                ),
            ));
        }

        // Seal the exact bytes before anything is derived from them, so a
        // reviewer can re-run the parser over the same input and check every
        // content hash against it.
        let response_ref = format!("responses/{}.json", file_stem(&supply_plan.name));
        self.directory
            .write_bytes(&response_ref, &acquisition.raw_response)?;
        let response_hash = sha256_digest(&acquisition.raw_response);
        // A replayed acquisition seals the *recorded* endpoint: the loopback
        // origin's ephemeral port is transport plumbing that would make two
        // replay runs of one suite disagree, and the recorded endpoint is the
        // honest answer to what these bytes are a response to. The replay
        // record below keeps the loopback fact ("served over loopback" in the
        // dossier) without keeping the port.
        let recording = self.options.replay.as_ref().map(|replay| {
            replay
                .recordings
                .get(&provider)
                .expect("execute() refuses a replay run with an unrecorded provider")
        });
        let endpoint = match recording {
            Some(recording) => recording.recorded_endpoint.clone(),
            None => acquisition.endpoint.clone(),
        };
        let mut acquisition_json = json!({
            "endpoint": endpoint,
            "http_status": acquisition.http_status,
            "latency_ms": acquisition.latency_ms,
            "provider_request_id": acquisition.provider_request_id,
            "charge": acquisition.charge,
            "response_ref": response_ref,
            "response_hash": response_hash,
            "response_bytes": acquisition.raw_response.len(),
            "result_count": acquisition.envelopes.len(),
        });
        if let Some((record, ..)) = &quote {
            acquisition_json["quote"] = record.clone();
        }
        // The dispatch gate's account of this acquisition: the reservation
        // settled against the receipt, or the observed charge committed with
        // nothing reserved, and the explicit absence where no context was
        // held. Kept beside the charge it accounts for.
        if let Some((mut record, reservation)) = dispatch_allowance.take() {
            if let Some(allowance) = self.options.allowance.as_ref()
                && record["consulted"].as_bool() == Some(true)
                && let Err(failure) = allowance.settle_success(
                    &mut record,
                    reservation.as_ref(),
                    &acquisition.charge,
                    Utc::now(),
                    &format!("run {}", self.run_id),
                )
            {
                plan.add_gap(Gap::new(GapReason::EvidenceMissing, failure.to_string()));
            }
            acquisition_json["allowance"] = record;
        }
        // What was executed, under whose declaration, and what it exited
        // with. Present only where something was executed; a content
        // acquisition carries no `invocation` key at all, because an empty
        // one would suggest a process nobody spawned. The step's result limit
        // rides along stated rather than applied: one invocation produces one
        // result, and a reader comparing a skill plan with a search plan
        // asking for four needs to see that the number governed nothing here.
        if let Some(invocation) = &acquisition.invocation {
            acquisition_json["invocation"] =
                serde_json::to_value(invocation).unwrap_or(Value::Null);
            acquisition_json["invocation"]["requested_limit"] = json!(step.limit);
            acquisition_json["invocation"]["limit_applied"] = json!(false);
            acquisition_json["invocation"]["limit_note"] = json!(
                "The plan step's result limit does not govern an invocation: one invocation \
                 produces one result. It is recorded because the comparison holds it fixed \
                 across every plan."
            );
        }
        // In replay mode the bytes sealed above were served from a verified
        // recording, and the record says which one. `matches_recorded_input`
        // is computed, not asserted: the sealed hash either equals the
        // manifest's recorded hash or the record shows it does not.
        if let Some(recording) = recording {
            acquisition_json["replay"] = json!({
                "source": recording.source,
                "captured_at": recording.captured_at,
                "recorded_endpoint": recording.recorded_endpoint,
                "recorded_response_sha256": recording.response_sha256,
                "matches_recorded_input": response_hash == recording.response_sha256,
                "redactions": recording.redactions,
                "permitted_use": recording.permitted_use,
            });
        }

        // The date each admitted part will carry into the freshness record:
        // the supplier's own declaration from the envelope where the adapter
        // mapped one — the claim closest to the content — else the
        // recording's capture date when this plan was replayed. A live
        // acquisition whose supplier declared nothing has no date, and the
        // freshness record then says unmeasured rather than this runtime
        // reading a clock.
        let capture_date = recording.map(|recording| DeclaredDate {
            date: recording.captured_at.clone(),
            provenance: format!(
                "replay recording capture date, declared by the replay manifest for {}",
                recording.source
            ),
        });

        // For a quoted purchase the cost ruling is the purchase decision,
        // already taken and applied before any money moved; running the
        // post-hoc cap check again over the receipt would give one plan two
        // accounts of the same decision. Everything else is judged on the
        // charge it reported after the fact — the only point that exists.
        let cost = match &quote {
            Some((_, _, ruling)) => ruling.clone(),
            None => {
                let cost = policy::acquisition_cost(&self.suite.job, &acquisition.charge);
                plan.apply(&cost, self.run_id, &self.suite.job, None);
                cost
            }
        };
        // The supply leg's wall clock is every request that ran: the quote
        // legs where the plan quoted, then the settlement.
        let supply_latency_ms = acquisition
            .latency_ms
            .saturating_add(quote.as_ref().map(|(_, ms, _)| *ms).unwrap_or(0));

        // Admission is decided before the budget is fitted, and the budget is
        // fitted only over the sources policy lets through. A source refused by
        // admission consumes none of the budget it was never going to use;
        // counting it would drop an admissible later source with a gap claiming
        // the budget was spent when the context sent to the model says it was
        // not.
        let admissions: Vec<Ruling> = acquisition
            .envelopes
            .iter()
            .map(|envelope| {
                policy::source_admission_from(&self.suite.job, envelope, Some(&provider))
            })
            .collect();

        // The admit-stage support governor runs only when the suite declares
        // governance, over every source the checks above would let cross:
        // the suite's sealed rule set against the source's declared
        // metadata, before the model sees it. An ungoverned suite records no
        // invocation at all — the grounding evaluator's opt-in precedent.
        let mut support_rulings: Vec<Option<Ruling>> =
            Vec::with_capacity(acquisition.envelopes.len());
        for (envelope, admission) in acquisition.envelopes.iter().zip(&admissions) {
            let ruling = match (&self.suite.governance, &self.suite.as_of) {
                (Some(governance), Some(as_of))
                    if !admission.is_refusal() && !cost.is_refusal() =>
                {
                    let (invocation, ruling) = processor::support::invoke(
                        self.suite.job.policy_mode,
                        &envelope.source_url,
                        envelope.content_hash.as_deref(),
                        &envelope.native_metadata,
                        governance,
                        &as_of.date,
                    );
                    self.record_invocation(&mut plan, &invocation);
                    plan.apply(
                        &ruling,
                        self.run_id,
                        &self.suite.job,
                        Some(&envelope.source_url),
                    );
                    Some(ruling)
                }
                _ => None,
            };
            support_rulings.push(ruling);
        }

        // The admit-stage screens scan every source the checks above would let
        // cross: the PII detector, then the injection screen. Each verdict
        // takes the same `Ruling` shape as every other admission constraint —
        // strict refuses the source before the model sees it, observe and
        // prefer record the finding and carry on. They run in a fixed order so
        // the invocation records land deterministically; either refusing keeps
        // the source out of the window.
        let mut pii_rulings: Vec<Option<Ruling>> = Vec::with_capacity(acquisition.envelopes.len());
        let mut injection_rulings: Vec<Option<Ruling>> =
            Vec::with_capacity(acquisition.envelopes.len());
        for (index, (envelope, admission)) in
            acquisition.envelopes.iter().zip(&admissions).enumerate()
        {
            let governed_out = support_rulings[index]
                .as_ref()
                .is_some_and(Ruling::is_refusal);
            let screenable = (!admission.is_refusal() && !cost.is_refusal() && !governed_out)
                .then_some(envelope.text.as_deref())
                .flatten();
            let pii_ruling = screenable.map(|text| {
                // A run's sources come from a supply adapter: the operator's
                // own corpus is internal, and so is anything a supplier
                // handed back at a local or private address, by the same
                // floor the harness applies. Everything else is public. A
                // job declares no `refuse_on_pii`, so the switch is off here.
                let source = if acquisition.provider == commonmeasure_supply::INTERNAL_PROVIDER
                    || commonmeasure_types::address::is_private_address(&envelope.source_url)
                {
                    processor::pii::SourceClass::Internal
                } else {
                    processor::pii::SourceClass::Public
                };
                let (invocation, ruling) = processor::pii::invoke(
                    self.suite.job.policy_mode,
                    source,
                    false,
                    &envelope.source_url,
                    &envelope.source_url,
                    text,
                    envelope.content_hash.as_deref(),
                );
                self.record_invocation(&mut plan, &invocation);
                plan.apply(
                    &ruling,
                    self.run_id,
                    &self.suite.job,
                    Some(&envelope.source_url),
                );
                ruling
            });
            let injection_ruling = screenable.map(|text| {
                let (invocation, ruling) = processor::injection::invoke(
                    self.suite.job.policy_mode,
                    &envelope.source_url,
                    &envelope.source_url,
                    text,
                    envelope.content_hash.as_deref(),
                );
                self.record_invocation(&mut plan, &invocation);
                plan.apply(
                    &ruling,
                    self.run_id,
                    &self.suite.job,
                    Some(&envelope.source_url),
                );
                ruling
            });
            pii_rulings.push(pii_ruling);
            injection_rulings.push(injection_ruling);
        }
        let usable = |index: usize| {
            !admissions[index].is_refusal()
                && !cost.is_refusal()
                && support_rulings[index]
                    .as_ref()
                    .is_none_or(|ruling| !ruling.is_refusal())
                && pii_rulings[index]
                    .as_ref()
                    .is_none_or(|ruling| !ruling.is_refusal())
                && injection_rulings[index]
                    .as_ref()
                    .is_none_or(|ruling| !ruling.is_refusal())
        };

        // The transform stage runs over exactly the sources admission let
        // through: deduplicating against text that will never enter the
        // window would remove content on the authority of content the model
        // does not see.
        let optimisable: Vec<(&commonmeasure_types::ContextEnvelope, &str)> = acquisition
            .envelopes
            .iter()
            .enumerate()
            .filter_map(|(index, envelope)| {
                let text = envelope.text.as_deref()?;
                usable(index).then_some((envelope, text))
            })
            .collect();
        let transformation = (!optimisable.is_empty()).then(|| {
            let (invocation, transformation) = processor::optimise::invoke(&optimisable);
            self.record_invocation(&mut plan, &invocation);
            transformation
        });
        let kept = transformation
            .as_ref()
            .map(|transformation| transformation.kept.as_slice())
            .unwrap_or_default();
        let mut fitted = fit_context_budget(
            kept.iter()
                .map(|source| (source.envelope.source_url.as_str(), source.tokens_out)),
            self.suite.job.maximum_context_tokens(),
        )
        .into_iter();
        let mut kept_iter = kept.iter();
        let mut dropped_iter = transformation
            .as_ref()
            .map(|transformation| transformation.dropped.iter())
            .into_iter()
            .flatten()
            .peekable();

        let mut sources = Vec::new();
        let mut parts = Vec::new();
        let mut part_dates = Vec::new();
        let mut admitted_tokens = 0u64;
        for (index, (envelope, admission)) in
            acquisition.envelopes.iter().zip(&admissions).enumerate()
        {
            plan.apply(
                admission,
                self.run_id,
                &self.suite.job,
                Some(&envelope.source_url),
            );
            let support = support_rulings[index].as_ref();
            let pii = pii_rulings[index].as_ref();
            let injection = injection_rulings[index].as_ref();
            let original_tokens = envelope
                .text
                .as_deref()
                .map(policy::approximate_tokens)
                .unwrap_or(0);

            let duplicate = dropped_iter
                .next_if(|dropped| std::ptr::eq(dropped.envelope, envelope))
                .map(|dropped| dropped.duplicate_of.clone());
            let (tokens, admitted, reason) = if !usable(index) {
                (
                    original_tokens,
                    false,
                    admission_reason(
                        admission,
                        support,
                        pii,
                        injection,
                        "Not admitted; the acquisition was refused before any source could cross.",
                    ),
                )
            } else if let Some(kept_url) = duplicate {
                (original_tokens, false, deduplicated_reason(&kept_url))
            } else {
                let source = kept_iter
                    .next()
                    .expect("every usable, non-duplicate source has a transform outcome");
                let outcome = fitted
                    .next()
                    .expect("every kept source has a budget outcome");
                // A source whose text was wholly deduplicated away is a
                // whole-source duplicate arrived at span by span, and zero
                // tokens fit any budget. Admitting it would publish the hash of
                // the empty string under its URL, and a citation of that URL
                // could then only ever be judged contradicted.
                let emptied = source.text.trim().is_empty();
                let carrier = source
                    .removed_spans
                    .first()
                    .map(|span| span.first_kept.reference.as_str());
                let budget_detail = outcome.gap.as_ref().map(|gap| gap.detail.clone());
                if let Some(gap) = outcome.gap {
                    plan.add_gap(gap);
                }
                if outcome.admitted && !emptied {
                    admitted_tokens += outcome.tokens;
                    parts.push(ContextPart {
                        source_ref: envelope.source_url.clone(),
                        // The hash of what actually enters the window: the
                        // transformed text. The invocation record ties it back
                        // to the envelope's own hash.
                        content_hash: source.output_hash.clone(),
                        text: source.text.clone(),
                    });
                    // The supplier's declared date outranks the capture date:
                    // it is the claim about the content, where the capture
                    // date only bounds when these bytes were fetched.
                    part_dates.push(
                        envelope
                            .declared_date
                            .clone()
                            .or_else(|| capture_date.clone()),
                    );
                }
                match (emptied, carrier, budget_detail) {
                    (true, Some(carrier), _) => {
                        (original_tokens, false, deduplicated_reason(carrier))
                    }
                    (true, None, _) => (
                        original_tokens,
                        false,
                        admission_reason(
                            admission,
                            support,
                            pii,
                            injection,
                            "Not admitted; the source's text is entirely whitespace, so there \
                             would be nothing behind a citation of it.",
                        ),
                    ),
                    // The budget gap already states which source needed what
                    // and how much of the job's allowance was left. Restating
                    // it here in other words is how one plan comes to carry two
                    // accounts of the same decision.
                    (false, _, Some(detail)) => (
                        outcome.tokens,
                        false,
                        admission_reason(
                            admission,
                            support,
                            pii,
                            injection,
                            &format!("Not admitted; {detail}"),
                        ),
                    ),
                    (false, _, None) => (
                        outcome.tokens,
                        outcome.admitted,
                        admission_reason(
                            admission,
                            support,
                            pii,
                            injection,
                            "Admitted; no constraint excluded it.",
                        ),
                    ),
                }
            };

            sources.push(json!({
                "url": envelope.source_url,
                "host": envelope.host,
                "title": envelope.title,
                "content_hash": envelope.content_hash,
                "licence": envelope.licence,
                "retrieval_rank": envelope.retrieval_rank,
                "tokens": tokens,
                "admitted": admitted,
                "admission_reason": reason,
                "native_metadata": envelope.native_metadata,
            }));
        }

        if parts.is_empty() {
            plan.add_gap(Gap::new(
                GapReason::EvidenceMissing,
                "No source from this provider was admitted, so inference was not attempted.",
            ));
            let status = if cost.is_refusal() {
                "refused"
            } else {
                "unavailable"
            };
            let mut value = plan.finish(
                status,
                acquisition_json,
                sources,
                Some(supply_latency_ms),
                Value::Null,
            );
            // Nothing was admitted, which is a known zero rather than an
            // unmeasured field: the acquisition happened and policy used none
            // of it.
            value["context_tokens_admitted"] = json!(0);
            // The call still completed and its bytes are sealed above.
            // Reporting `planned` here would deny a call the same plan's
            // acquisition record — and possibly its charge — proves happened.
            value["verification_state"] = json!(self.verification_state());
            // The acquisition completed and admission ran, so the empty
            // window is this plan's measured state, not an absence.
            return Ok(self.record_plan(
                value,
                Some(&Window {
                    parts: &parts,
                    dates: &part_dates,
                }),
            ));
        }

        let (mut status, inference) = match self.infer(&parts) {
            Ok(inference) => ("completed", inference),
            Err(gap) => {
                plan.add_gap(gap);
                ("unavailable", Value::Null)
            }
        };
        // A plan's latency is the sum of the legs that ran. When inference did
        // not run there is no inference leg to add — which is not the same as
        // adding a zero-length one, and writing it as `unwrap_or(0)` makes an
        // absent measurement arithmetically indistinguishable from an
        // instantaneous one the moment any leg's timing becomes optional.
        let total_latency = match inference.get("latency_ms").and_then(Value::as_u64) {
            Some(inference_ms) => supply_latency_ms.saturating_add(inference_ms),
            None => supply_latency_ms,
        };
        let latency = policy::total_latency(&self.suite.job, total_latency);
        plan.apply(&latency, self.run_id, &self.suite.job, None);
        if latency.is_refusal() {
            status = "refused";
        }

        let mut value = plan.finish(
            status,
            acquisition_json,
            sources,
            Some(total_latency),
            inference,
        );
        value["context_tokens_admitted"] = json!(admitted_tokens);
        value["verification_state"] = json!(self.verification_state());
        Ok(self.record_plan(
            value,
            Some(&Window {
                parts: &parts,
                dates: &part_dates,
            }),
        ))
    }

    /// Quote, decide, and — only if the decision allows — buy.
    ///
    /// The order is the contract: both quote legs are sealed and the quote
    /// record built *before* the purchase decision is taken, so the price and
    /// billing note the decision rested on are in evidence whichever way it
    /// goes. Nothing in this method spends; the one spending call is the
    /// final `confirm`, reached only through an allowing ruling.
    fn quoted_purchase(
        &mut self,
        plan: &mut PlanUnderConstruction,
        supply_plan: &SupplyPlan,
    ) -> Result<QuoteStep, RunError> {
        let provider = &supply_plan.steps[0].provider.name;
        let supplier = match (self.options.suppliers)(provider) {
            Ok(supplier) => supplier,
            Err(error) => return Ok(QuoteStep::Failed(error)),
        };
        let quote = match supplier.quote(&self.suite.job.prompt, supply_plan.steps[0].limit) {
            Ok(quote) => quote,
            Err(error) => return Ok(QuoteStep::Failed(error)),
        };

        // Seal both quote legs before anything is decided from them, so a
        // reviewer can re-derive the quoted price from the same bytes.
        let inspect_ref = format!("responses/{}-inspect.json", file_stem(&supply_plan.name));
        let preview_ref = format!("responses/{}-quote.json", file_stem(&supply_plan.name));
        self.directory
            .write_bytes(&inspect_ref, &quote.raw_inspect)?;
        self.directory
            .write_bytes(&preview_ref, &quote.raw_preview)?;

        let trial_covered = quote.billing_status.as_deref() == Some("trial");
        let mut record = json!({
            "endpoint": quote.endpoint,
            "preview_id": quote.preview_id,
            "workflow": quote.workflow,
            "billing_status": quote.billing_status,
            "billing_note": quote.billing_note,
            "trial_remaining": quote.trial_remaining,
            "trial_total": quote.trial_total,
            "balance_before": quote.balance_before,
            "charge": quote.charge,
            "latency_ms": quote.latency_ms,
            "inspect_ref": inspect_ref,
            "inspect_hash": sha256_digest(&quote.raw_inspect),
            "preview_ref": preview_ref,
            "preview_hash": sha256_digest(&quote.raw_preview),
        });

        // Only the recorded workflow is buyable: `requires_confirm` is the
        // one shape whose settlement this runtime has evidence for. Anything
        // else — including the documented free-and-already-unlocked
        // `complete` — is a state with no recorded settlement path, and
        // guessing one would spend or claim on a shape nobody has seen.
        if quote.workflow != "requires_confirm" {
            let detail = format!(
                "The preview answered workflow \"{}\"; the only settlement path this runtime \
                 has recorded evidence for is \"requires_confirm\", so nothing was bought and \
                 no content was taken from the preview.",
                quote.workflow
            );
            plan.add_gap(Gap::new(GapReason::EvidenceMissing, detail.clone()));
            record["decision"] = json!("not_attempted");
            record["decision_reason"] = json!(detail);
            return Ok(QuoteStep::NotPurchased {
                record,
                latency_ms: quote.latency_ms,
                status: "unavailable",
            });
        }

        let ruling = policy::purchase_decision(&self.suite.job, &quote.charge, trial_covered);
        plan.apply(&ruling, self.run_id, &self.suite.job, None);
        let reason = match (&ruling, trial_covered) {
            (Ruling::Allowed, true) => {
                "The supplier states the purchase is covered by the trial; no charge applies \
                 (the billing note above is the supplier's own sentence)."
                    .to_owned()
            }
            (Ruling::Allowed, false) => "No constraint declined the purchase.".to_owned(),
            (ruling, _) => ruling.reason().unwrap_or_default().to_owned(),
        };
        record["decision_reason"] = json!(reason);
        if ruling.is_refusal() {
            record["allowance"] = json!({
                "consulted": false,
                "reason": "the job's own cost cap declined the purchase, so the allowance \
                           was never reached",
            });
            record["decision"] = json!("declined");
            return Ok(QuoteStep::NotPurchased {
                record,
                latency_ms: quote.latency_ms,
                status: "refused",
            });
        }

        // The principal's cumulative allowance is the second pre-spend gate,
        // and a separate record from the job cap above: one bounds this
        // purchase, the other the period's total.
        let (allowance_record, reservation, allowance_ruling) =
            self.allowance_gate(&quote, trial_covered, &format!("run {}", self.run_id));
        record["allowance"] = allowance_record;
        if let Some(allowance_ruling) = allowance_ruling {
            plan.apply(&allowance_ruling, self.run_id, &self.suite.job, None);
            if allowance_ruling.is_refusal() {
                record["decision"] = json!("declined");
                record["decision_reason"] = json!(allowance_ruling.reason().unwrap_or_default());
                return Ok(QuoteStep::NotPurchased {
                    record,
                    latency_ms: quote.latency_ms,
                    status: "refused",
                });
            }
        }

        record["decision"] = json!("confirmed");
        let acquisition = supplier.confirm(&quote);
        if let Some(allowance) = self.options.allowance.as_ref() {
            let now = Utc::now();
            let context = format!("run {}", self.run_id);
            // Either outcome writes its own `settlement`; a ledger that took
            // neither write is a gap in the plan, stated in the ledger's terms.
            let gap = match &acquisition {
                Ok(settled) => allowance
                    .settle_success(
                        &mut record["allowance"],
                        reservation.as_ref(),
                        &settled.charge,
                        now,
                        &context,
                    )
                    .err()
                    .map(|failure| failure.to_string()),
                Err(error) => reservation.as_ref().and_then(|reservation| {
                    allowance
                        .release_after_failure(
                            &mut record["allowance"],
                            reservation,
                            &format!("the confirm failed before settlement ({error})"),
                            now,
                        )
                        .err()
                        .map(|release_error| {
                            format!(
                                "The allowance reservation could not be released: {release_error}"
                            )
                        })
                }),
            };
            if let Some(detail) = gap {
                plan.add_gap(Gap::new(GapReason::EvidenceMissing, detail));
            }
        }
        Ok(QuoteStep::Purchased {
            acquisition: Box::new(acquisition),
            record,
            latency_ms: quote.latency_ms,
            ruling,
        })
    }

    /// Consult the principal's cumulative allowance ahead of a purchase.
    ///
    /// Returns the evidence record of the consultation, the reservation now
    /// held (to be settled against the receipt), and a ruling when the gate
    /// found anything to say — a refusal in strict, a recorded breach in
    /// observe and prefer, nothing when the purchase simply fits. Absence is
    /// recorded, never inferred: a run without an allowance context says so.
    fn allowance_gate(
        &self,
        quote: &SupplyQuote,
        trial_covered: bool,
        context: &str,
    ) -> (Value, Option<Reservation>, Option<Ruling>) {
        let Some(allowance) = self.options.allowance.as_ref() else {
            return (
                json!({
                    "consulted": false,
                    "reason": "the run holds no principal allowance context: it was started \
                               without principal policy, or in replay, where no money moves",
                }),
                None,
                None,
            );
        };
        let mut record = json!({
            "consulted": true,
            "principal": allowance.principal,
            "declared": !allowance.declarations.is_empty(),
        });
        if allowance.declarations.is_empty() {
            record["reason"] = json!(format!(
                "principal {:?} declares no allowance",
                allowance.principal
            ));
            return (record, None, None);
        }
        if trial_covered {
            record["decision"] = json!("nothing_reserved");
            record["reason"] = json!(
                "the supplier states this purchase is trial-covered; no money moves, so \
                 nothing is reserved"
            );
            return (record, None, None);
        }
        let Some(price) = quote.charge.money.as_ref() else {
            let quoted_as = match quote.charge.native.as_ref() {
                Some(native) => format!("{} {}", native.amount, native.unit),
                None => "no price at all".to_owned(),
            };
            let ruling = Ruling::breach(
                self.suite.job.policy_mode,
                format!(
                    "Principal {:?} holds a cumulative allowance and the quote is {quoted_as}, \
                     which a monetary allowance cannot be checked against; buying at an \
                     unverifiable price is declined rather than discovered on the receipt.",
                    allowance.principal
                ),
                Gap::new(
                    GapReason::EvidenceMissing,
                    "The quoted price could not be verified against the principal's cumulative \
                     allowance before purchase.",
                ),
            );
            record["decision"] = json!(if ruling.is_refusal() {
                "declined"
            } else {
                "proceeded_with_breach"
            });
            record["reason"] = json!(ruling.reason().unwrap_or_default());
            return (record, None, Some(ruling));
        };
        let decision =
            allowance.reserve_decision(price, self.suite.job.policy_mode, Utc::now(), context);
        (decision.record, decision.reservation, decision.ruling)
    }

    /// What a completed, sealed acquisition earns (`docs/contracts/provider.md`).
    ///
    /// A dated call against the real supply, its exact response bytes sealed
    /// beside the run, is `live-verified` — authenticated where the provider
    /// is remote. The same path fed from a verified recording is
    /// `replay-tested`: the code is exercised end to end, but no call reached
    /// the provider, and the two claims must never be confusable. Nothing
    /// weaker earns either.
    fn verification_state(&self) -> &'static str {
        if self.options.replay.is_some() {
            "replay-tested"
        } else {
            "live-verified"
        }
    }

    /// Run inference, or return the gap that explains why there is no answer.
    fn infer(&self, context: &[ContextPart]) -> Result<Value, Gap> {
        let Some(backend) = self.backend else {
            return Err(Gap::new(
                GapReason::InferenceUnavailable,
                format!(
                    "No inference gateway is configured. Set {} to an OpenAI-compatible \
                     chat-completions endpoint.",
                    commonmeasure_inference::ENDPOINT_VARIABLE
                ),
            ));
        };
        let request = InferenceRequest {
            run_id: self.run_id,
            model_plan: self.suite.model_plan.clone(),
            system: self.suite.system_prompt(),
            prompt: self.suite.job.prompt.clone(),
            context: context.to_vec(),
        };
        match backend.infer(&request) {
            Ok(response) => Ok(json!({
                "status": "completed",
                "gateway": backend.name(),
                "requested_model": response.requested_model,
                "executed_model": response.executed_model,
                "executed_provider": response.executed_provider,
                "route_reason": response.route_reason,
                "latency_ms": response.latency_ms,
                "input_tokens": response.input_tokens,
                "output_tokens": response.output_tokens,
                "provider_request_id": response.provider_request_id,
                "unavailable_fields": response.unavailable_fields,
                "unreadable_fields": response.unreadable_fields,
                "answer": response.output,
            })),
            Err(error) => Err(Gap::new(
                match error {
                    InferenceError::Unavailable(_) => GapReason::InferenceUnavailable,
                    _ => GapReason::EvidenceMissing,
                },
                error.to_string(),
            )),
        }
    }

    /// Evaluate the plan and append its `plan_completed` record. Every plan
    /// passes through here, so every record — refused and unavailable plans
    /// included — carries every evaluator's section stating what was
    /// measured or why nothing was measurable; the evaluation sits inside
    /// the recorded payload, so the append-only trail and the summary carry
    /// one value, computed once.
    ///
    /// `window: None` means no admitted window was assembled (the
    /// acquisition never completed); an assembled window that admitted
    /// nothing passes `Some` with empty parts, because "bought and admitted
    /// nothing" is a fact about the plan where "never acquired" is not.
    fn record_plan(&mut self, mut plan: Value, window: Option<&Window>) -> Value {
        let parts = window.map(|window| window.parts);
        let grounding = evaluate::grounding(&evaluate::EvaluationInput {
            answer: plan["answer"].as_str(),
            cited_answer_required: self.suite.require_cited_answer,
            parts: parts.unwrap_or(&[]),
        });
        let coverage = coverage::coverage(&coverage::CoverageInput {
            rubric: self.suite.coverage_rubric.as_ref(),
            window: parts,
        });
        let dated: Option<Vec<freshness::DatedPart>> = window.map(|window| {
            debug_assert_eq!(
                window.parts.len(),
                window.dates.len(),
                "parts and their dates are built together, one date slot per part"
            );
            window
                .parts
                .iter()
                .zip(window.dates)
                .map(|(part, date)| freshness::DatedPart {
                    reference: &part.source_ref,
                    content_hash: &part.content_hash,
                    declared: date.as_ref(),
                })
                .collect()
        });
        let freshness = freshness::freshness(&freshness::FreshnessInput {
            as_of: self.suite.as_of.as_ref(),
            window: dated.as_deref(),
        });
        // The verify stage runs over the answer once the grounding record
        // exists, so its per-claim evidence can cite the citation verdicts.
        // An answer that exists is verified whatever the window holds; the
        // baseline's empty window makes every claim unsupported, which is
        // the fact about it.
        if let (Some(answer), Some(parts)) = (plan["answer"].as_str(), parts)
            && !answer.trim().is_empty()
        {
            let (verifier, claims) = processor::fidelity::invoke(answer, &grounding, parts);
            self.append_invocation(&mut plan, &verifier);
            if self.suite.fidelity_judge {
                let judge = processor::judge::invoke(
                    self.backend,
                    self.run_id,
                    &self.suite.model_plan,
                    parts,
                    &claims,
                );
                self.append_invocation(&mut plan, &judge);
            }
        }
        // The attest stage runs last, once the verify records exist, so the
        // plan's invocations stand in the contract's stage order.
        if let Some(parts) = parts {
            self.label_output(&mut plan, parts);
        }
        plan["evaluation"] = json!({
            "grounding": grounding,
            "coverage": coverage,
            "freshness": freshness,
        });
        record(self.log, "plan_completed", plan.clone());
        plan
    }

    /// A verify-stage invocation on a plan already assembled: appended to the
    /// evidence log and to the plan's `processors`, in order, like every
    /// earlier stage's.
    fn append_invocation(&mut self, plan: &mut Value, invocation: &processor::Invocation) {
        let value = invocation.to_value();
        record(self.log, "processor_invoked", value.clone());
        if let Some(processors) = plan["processors"].as_array_mut() {
            processors.push(value);
        }
    }

    /// The attest stage: when the suite declares `output_provenance` and the
    /// plan has an answer, build the C2PA manifest from this plan's window
    /// (each part by the hash of the text that entered, every one a mediated
    /// crossing because the run carried it under policy), sign it, embed it
    /// in the answer and publish the labelled text beside the raw manifest
    /// store. The plan's `answer` stays the model's own bytes, because the
    /// evaluators read it; the label is a separate artefact the invocation
    /// record names.
    fn label_output(&mut self, plan: &mut Value, parts: &[ContextPart]) {
        let Some(policy) = &self.suite.output_provenance else {
            return;
        };
        let Some(answer) = plan["answer"].as_str().map(str::to_owned) else {
            return;
        };
        let plan_id = plan["id"].as_str().unwrap_or_default().to_owned();
        let sources: Vec<provenance::Source> = parts
            .iter()
            .map(|part| provenance::Source {
                reference: part.source_ref.clone(),
                content_hash: part.content_hash.clone(),
                grade: Grade::Mediated,
            })
            .collect();
        let stem = file_stem(&plan_id);
        let labelled_reference = format!("provenance/{stem}.txt");
        let manifest_reference = format!("provenance/{stem}.c2pa");
        let run_id = self.run_id.to_string();
        let answer_reference = format!("plans/{plan_id}/answer");
        let (invocation, label) = provenance::invoke(&provenance::Input {
            run_id: &run_id,
            plan_id: &plan_id,
            run_manifest_hash: self.manifest_hash,
            answer: &answer,
            answer_reference: &answer_reference,
            sources: &sources,
            policy,
            signing: &self.options.provenance_signing,
            labelled_reference: &labelled_reference,
            manifest_reference: &manifest_reference,
        });
        self.append_invocation(plan, &invocation);
        if let Some(label) = label {
            let written = self
                .directory
                .write_bytes(&labelled_reference, label.text.as_bytes())
                .and_then(|_| {
                    self.directory
                        .write_bytes(&manifest_reference, &label.manifest)
                });
            if let Err(error) = written
                && let Some(gaps) = plan["gaps"].as_array_mut()
            {
                gaps.push(json!(Gap::new(
                    GapReason::WriteFailed,
                    format!(
                        "The provenance label was built and read back but could not be \
                         published to {labelled_reference}: {error}"
                    ),
                )));
            }
        }
    }

    /// One processor invocation: appended to the evidence log as its own
    /// record and carried in the plan, so a reader finds it both in the
    /// append-only trail and beside the sources it judged.
    fn record_invocation(
        &mut self,
        plan: &mut PlanUnderConstruction,
        invocation: &processor::Invocation,
    ) {
        let value = invocation.to_value();
        record(self.log, "processor_invoked", value.clone());
        plan.processors.push(value);
    }
}

/// Where a quote-then-buy plan got to before the shared admission pipeline.
enum QuoteStep {
    /// The quote itself failed; no purchase decision ever existed.
    Failed(SupplyError),
    /// The quote is in evidence and the purchase was not made — declined by
    /// policy (`refused`) or unattemptable (`unavailable`); the record's
    /// `decision` and `decision_reason` say which.
    NotPurchased {
        record: Value,
        latency_ms: u64,
        status: &'static str,
    },
    /// The purchase was confirmed. `acquisition` is the settlement's outcome
    /// — the buy itself can still fail — and `ruling` is the purchase
    /// decision, which stands as the plan's cost ruling.
    Purchased {
        acquisition: Box<Result<Acquisition, SupplyError>>,
        record: Value,
        latency_ms: u64,
        ruling: Ruling,
    },
}

/// A plan's decisions and gaps as they accumulate.
struct PlanUnderConstruction {
    id: String,
    provider: String,
    capability: Option<ProviderCapability>,
    adapter_version: Option<String>,
    eligibility_reason: String,
    eligible: bool,
    decisions: Vec<Value>,
    gaps: Vec<Gap>,
    processors: Vec<Value>,
}

impl PlanUnderConstruction {
    fn new(id: &str, provider: &str, step: Option<&SupplyStep>) -> Self {
        Self {
            id: id.to_owned(),
            provider: provider.to_owned(),
            capability: step.map(|step| step.capability),
            adapter_version: step.map(|step| step.provider.adapter_version.clone()),
            eligibility_reason: "No constraint excluded this provider.".to_owned(),
            eligible: true,
            decisions: Vec::new(),
            gaps: Vec::new(),
            processors: Vec::new(),
        }
    }

    fn add_gap(&mut self, gap: Gap) {
        self.gaps.push(gap);
    }

    /// Record a ruling: its gap where it has one, and a decision record naming
    /// what was decided and why.
    fn apply(&mut self, ruling: &Ruling, run_id: Uuid, job: &ContextJob, source: Option<&str>) {
        let (Some(reason), Some(gap)) = (ruling.reason(), ruling.gap()) else {
            return;
        };
        self.gaps.push(gap.clone());
        if source.is_none() {
            self.eligible = !ruling.is_refusal();
            self.eligibility_reason = reason.to_owned();
        }
        let record = DecisionRecord {
            id: Uuid::new_v4(),
            job_id: job.id,
            run_id,
            timestamp: Utc::now(),
            decision: if ruling.is_refusal() {
                Decision::Refuse
            } else {
                Decision::Admit
            },
            reason: reason.to_owned(),
            // Every decision rests on bytes this runtime saw or on the job's own
            // declared policy, never on a supplier's assurance about itself.
            assurance: AssuranceBasis::Observed,
            plan: Some(self.id.clone()),
            provider: Some(self.provider.clone()),
            source_url: source.map(str::to_owned),
            model: None,
            observed_cost: None,
            gaps: vec![gap.clone()],
        };
        debug_assert!(
            record.validate().is_ok(),
            "a decision needs a reason, and a refusal needs a gap"
        );
        self.decisions
            .push(serde_json::to_value(record).unwrap_or(Value::Null));
    }

    fn finish(
        self,
        status: &str,
        acquisition: Value,
        sources: Vec<Value>,
        latency_ms: Option<u64>,
        inference: Value,
    ) -> Value {
        let admitted = sources
            .iter()
            .filter(|source| source["admitted"] == Value::Bool(true))
            .count();
        json!({
            "id": self.id,
            "provider": self.provider,
            "capability": self.capability,
            "adapter_version": self.adapter_version,
            // Nothing this run did supports a verification claim unless it
            // completed an authenticated call and sealed the response; the
            // caller raises this only in that case.
            "verification_state": "planned",
            "status": status,
            "eligibility": {"eligible": self.eligible, "reason": self.eligibility_reason},
            "acquisition": acquisition,
            "latency_ms": latency_ms,
            "sources": sources,
            "source_count": admitted,
            "context_tokens_admitted": Value::Null,
            "answer": inference.get("answer").cloned().unwrap_or(Value::Null),
            "inference": inference,
            "policy_decisions": self.decisions,
            // Every processor invocation this plan caused, in order. Empty is
            // the honest state for a plan that admitted nothing scannable.
            "processors": self.processors,
            "gaps": self.gaps,
        })
    }
}

/// The reason published against one source. A recorded breach speaks first —
/// it is the constraint the operator declared — in stage order: the job's
/// own admission policy, then the governance verdict, then the PII finding.
/// `otherwise` carries what the caller knows about a source no breach
/// decided.
fn admission_reason(
    admission: &Ruling,
    support: Option<&Ruling>,
    pii: Option<&Ruling>,
    injection: Option<&Ruling>,
    otherwise: &str,
) -> String {
    admission
        .reason()
        .or_else(|| support.and_then(Ruling::reason))
        .or_else(|| pii.and_then(Ruling::reason))
        .or_else(|| injection.and_then(Ruling::reason))
        .unwrap_or(otherwise)
        .to_owned()
}

/// A source the optimiser left with nothing to contribute, whether it was
/// dropped whole or emptied span by span. The text a citation would point at is
/// in `carrier`, not here.
fn deduplicated_reason(carrier: &str) -> String {
    format!(
        "Not admitted; {} removed its text as a duplicate of {carrier}, which is kept earlier in \
         this plan (see the plan's processor invocations).",
        processor::optimise::NAME
    )
}

/// The plans the router may rank: never the baseline (it is the control, not
/// a route) and never a refused plan (policy already decided against it). A
/// completed plan is always a candidate; an unavailable plan is one only
/// where its window was assembled and evaluated — it bought and admitted
/// context this run measured, which is exactly what a coverage- or
/// freshness-weighted objective ranks, answer or no answer. The arms that
/// need an answer filter on `completed` themselves.
///
/// A plan that neither ran to an answer nor carries a measured fraction — a
/// failed or never-attempted acquisition — is not a candidate at all, and is
/// returned by id so the selection explanation can say so: a populated
/// ranking otherwise reads as proof that every requested provider took part.
fn candidates(plans: &[Value]) -> (Vec<Candidate>, Vec<String>) {
    let mut ranked = Vec::new();
    let mut excluded = Vec::new();
    for plan in plans
        .iter()
        .filter(|plan| plan["provider"] != "none" && plan["status"] != "refused")
    {
        let completed = plan["status"] == "completed";
        let coverage = measured_coverage(plan);
        let freshness = plan["evaluation"]["freshness"]["fraction"].as_f64();
        if !completed && coverage.is_none() && freshness.is_none() {
            excluded.push(plan["id"].as_str().unwrap_or_default().to_owned());
            continue;
        }
        ranked.push(Candidate {
            plan_id: plan["id"].as_str().unwrap_or_default().to_owned(),
            completed,
            latency_ms: plan["latency_ms"].as_u64(),
            observed_cost: serde_json::from_value(plan["acquisition"]["charge"]["money"].clone())
                .ok()
                .flatten(),
            coverage,
            freshness,
        });
    }
    (ranked, excluded)
}

/// Name the plans `candidates` left out. `unavailable_inputs` names
/// measurements, never plans, so prose is the one field the record has for
/// an exclusion — and without it a selection over the measured plans is
/// silent about a requested provider that failed, exactly the outage that
/// biases a ranking toward whoever succeeded.
fn name_excluded(selection: &mut Selection, excluded: &[String]) {
    if excluded.is_empty() {
        return;
    }
    let one = excluded.len() == 1;
    selection.explanation.push_str(&format!(
        " {} {} not {}: {} neither ran to an answer nor carr{} a measured window fraction; the \
         reason is in the plan's own status and gaps.",
        excluded.join(", "),
        if one { "was" } else { "were" },
        if one { "a candidate" } else { "candidates" },
        if one { "it" } else { "they" },
        if one { "ies" } else { "y" },
    ));
}

/// A plan's measured coverage, carried with the rubric identity it was
/// measured against — the fraction never travels without its basis.
fn measured_coverage(plan: &Value) -> Option<MeasuredCoverage> {
    let record = &plan["evaluation"]["coverage"];
    Some(MeasuredCoverage {
        fraction: record["fraction"].as_f64()?,
        rubric: format!(
            "{}/{}",
            record["rubric"]["name"].as_str()?,
            record["rubric"]["version"].as_str()?
        ),
    })
}

/// One plan per named provider, each a single step at the fixed limit, using
/// the acquisition capability the adapter declares: open-web providers
/// `search`, the internal corpus `query`, a catalogued skill `invoke`. They
/// are not interchangeable — a corpus query is not a web search, and neither
/// is running somebody's program — which is exactly why the step records
/// which one the plan performed.
fn supply_plans(suite: &Suite) -> Result<Vec<SupplyPlan>, RunError> {
    suite
        .providers
        .iter()
        .map(|provider| {
            let provider_ref =
                commonmeasure_supply::declared_provider_ref(provider).ok_or_else(|| {
                    RunError::Suite(format!(
                        "the suite names provider {provider}, which has no implemented adapter"
                    ))
                })?;
            // A suite that names a fetch target is a fetch comparison: every
            // provider must retrieve that exact URL, so the only capability
            // that fits is `fetch`, and a provider that does not declare it
            // cannot take part. Otherwise the run searches, and the capability
            // is the first acquisition one the provider declares.
            let capability = if suite.fetch_target.is_some() {
                if provider_ref
                    .capabilities
                    .contains(&ProviderCapability::Fetch)
                {
                    ProviderCapability::Fetch
                } else {
                    return Err(RunError::Suite(format!(
                        "the suite names a fetch_target, but provider {provider} declares no \
                         fetch capability, so it cannot retrieve a named URL"
                    )));
                }
            } else {
                [
                    ProviderCapability::Search,
                    ProviderCapability::Query,
                    ProviderCapability::Invoke,
                ]
                .into_iter()
                .find(|capability| provider_ref.capabilities.contains(capability))
                .ok_or_else(|| {
                    RunError::Suite(format!(
                        "provider {provider} declares no acquisition capability this runner \
                         can plan (search, query or invoke)"
                    ))
                })?
            };
            let plan = SupplyPlan {
                name: format!("{provider}-only"),
                version: "1".to_owned(),
                steps: vec![SupplyStep {
                    provider: provider_ref,
                    capability,
                    limit: suite.result_limit,
                }],
                maximum_cost: suite.job.maximum_acquisition_cost().cloned(),
            };
            plan.validate().map_err(|error| {
                RunError::Suite(format!("invalid supply plan for {provider}: {error:?}"))
            })?;
            Ok(plan)
        })
        .collect()
}

/// The published comparison a replay run owes: one binding per replayed plan,
/// naming the recorded source, the hash the manifest declared for its bytes,
/// and the hash of what this run actually sealed. The two hashes either agree
/// or the artefact shows they do not; nothing here is asserted without both
/// sides being on disk for a reader to recheck.
fn replay_bindings(replay: &crate::replay::ReplayContext, plans: &[Value]) -> Value {
    let bindings: Vec<Value> = plans
        .iter()
        .filter(|plan| plan["acquisition"]["replay"].is_object())
        .map(|plan| {
            let acquisition = &plan["acquisition"];
            let mut binding = json!({
                "plan": plan["id"],
                "provider": plan["provider"],
                "source": acquisition["replay"]["source"],
                "captured_at": acquisition["replay"]["captured_at"],
                "recorded_endpoint": acquisition["replay"]["recorded_endpoint"],
                "recorded_response_sha256": acquisition["replay"]["recorded_response_sha256"],
                "sealed_response_ref": acquisition["response_ref"],
                "sealed_response_hash": acquisition["response_hash"],
                "matches_recorded_input": acquisition["replay"]["matches_recorded_input"],
            });
            // A quoted plan sealed two more exchanges before its settlement;
            // each binds to its own recorded leg the same way, hash beside
            // hash, so the quote evidence is as recheckable as the receipt.
            if acquisition["quote"].is_object() {
                let provider = plan["provider"].as_str().unwrap_or_default();
                let legs: Vec<Value> = [
                    ("inspect", "inspect_ref", "inspect_hash"),
                    ("quote", "preview_ref", "preview_hash"),
                ]
                .iter()
                .filter_map(|(leg, reference, hash)| {
                    let recording = replay
                        .served
                        .get(provider)?
                        .iter()
                        .find(|recording| recording.capability == *leg)?;
                    let sealed_hash = &acquisition["quote"][*hash];
                    Some(json!({
                        "leg": leg,
                        "source": recording.source,
                        "recorded_response_sha256": recording.response_sha256,
                        "sealed_response_ref": acquisition["quote"][*reference],
                        "sealed_response_hash": sealed_hash,
                        "matches_recorded_input":
                            sealed_hash == &json!(recording.response_sha256),
                    }))
                })
                .collect();
                binding["quote_bindings"] = json!(legs);
            }
            binding
        })
        .collect();
    json!({
        "replay_version": "contextops-replay-run/v1",
        "manifest": replay.manifest_path,
        // The full provenance of every recording served — every leg of a
        // quote-then-buy provider included — with what was redacted before
        // commit and what serving it is for.
        "recordings": replay.served.values().flatten().collect::<Vec<_>>(),
        "bindings": bindings,
    })
}

const FAIRNESS: &str = "One job, one system prompt, one requested model, one result limit and one admission budget \
     across every plan. Each provider discovers independently; no provider is given another's \
     URLs. Acquisition charges stay in the provider's own unit.";

/// What is frozen for this comparison.
///
/// The inference endpoint is deliberately absent: it is deployment
/// configuration, and including it would make two runs of the same job on two
/// machines produce different manifest hashes. The requested model, which is
/// the experimental variable, is present.
/// How this run obtained its supply, in one place.
///
/// Read by the manifest and by the summary, so what identity seals and what a
/// reader is shown are necessarily the same word.
fn acquisition_mode(options: &RunOptions) -> &'static str {
    if options.replay.is_some() {
        "replay"
    } else if options.allow_external_acquisition {
        "live"
    } else {
        "no-external-acquisition"
    }
}

/// The recordings a replay run served, as one digest.
///
/// Over the recordings themselves, never the manifest path: the path is where
/// the operator happens to keep the directory, and sealing it would make two
/// replay runs of one suite disagree for a reason that is not about the
/// experiment.
fn replay_recordings_digest(options: &RunOptions) -> Option<String> {
    let replay = options.replay.as_ref()?;
    Some(canonical_digest(
        &serde_json::to_value(&replay.recordings).ok()?,
    ))
}

fn manifest(
    suite: &Suite,
    supply_plans: &[SupplyPlan],
    backend: Option<&dyn InferenceBackend>,
    options: &RunOptions,
) -> Value {
    // Sealed only when the suite names a fetch target, so a search suite's
    // manifest is byte-identical to before this field existed and no committed
    // hash moves. A fetch run is a different experiment, and its manifest says
    // so by carrying the exact URL every provider was asked to retrieve.
    let mut extra = suite
        .fetch_target
        .as_ref()
        .map(|url| json!({"fetch_target": url}))
        .unwrap_or_else(|| json!({}));
    // Sealed on the same terms: only a suite that declares the judge carries
    // the key, so every other manifest keeps its shape.
    if suite.fidelity_judge {
        extra["fidelity_judge"] = json!(true);
    }
    // Sealed on the same terms: only a suite that declares what may be done
    // with its outputs carries the key, and a suite that changes that
    // declaration is a different experiment by hash.
    if let Some(provenance) = &suite.output_provenance {
        extra["output_provenance"] = json!(provenance);
    }
    let mut processors = processor::installed();
    if suite
        .output_provenance
        .as_ref()
        .is_some_and(|policy| policy.encypher.is_some())
    {
        for installed in &mut processors {
            if installed.name == provenance::NAME {
                *installed = provenance::encypher::manifest();
            }
        }
    }
    let mut manifest = json!({
        // v2: the manifest seals the composed system prompt (the fixed
        // instruction plus the citation directive when the suite requires a
        // cited answer) and the installed evaluator identities.
        // v3: it seals the suite's declared coverage rubric and as-of
        // reference (both null when undeclared) — they are what the coverage
        // and freshness evaluators measure against, so a suite that declares
        // either is a different experiment by hash.
        // v4: it seals the suite's declared governance block (null when
        // undeclared) — the support-status rule set and entitlement grant
        // admission is decided under, so flipping one rule is a different
        // experiment by hash. That traceability is what the revocation
        // demonstration rests on: nothing else names the rules, because the
        // internal corpus itself is not sealed here.
        // A `fetch_target` key appears only in a fetch run (below), so search
        // manifests keep their v4 shape and hashes.
        "manifest_version": "contextops-manifest/v4",
        "suite_version": suite.suite_version,
        "label": suite.label,
        "job": suite.job,
        "supply_plans": supply_plans,
        "model_plan": suite.model_plan,
        "result_limit": suite.result_limit,
        "system_prompt": suite.system_prompt(),
        "coverage_rubric": suite.coverage_rubric,
        "as_of": suite.as_of,
        "governance": suite.governance,
        "adapter_version": commonmeasure_supply::ADAPTER_VERSION,
        // The installed processors are part of what is frozen: their rule
        // sets shape what enters the window, so their identity and
        // configuration digests belong beside the job they shaped.
        "processors": processors,
        // The evaluator set is fixed per experiment
        // (docs/contracts/experiment.md): what will judge the answers is
        // part of what the comparison is, so it is sealed with the job.
        "evaluators": evaluate::installed(),
        "token_basis": TOKEN_BASIS,
        // A run served from recordings, a run that asked for nothing and a
        // billable live run are three different experiments over one job, so
        // how the supply was obtained is part of what the hash identifies —
        // and in replay mode, which recordings answered.
        "acquisition_mode": acquisition_mode(options),
        "replay_recordings": replay_recordings_digest(options),
        "inference_gateway": backend.map(InferenceBackend::name),
        // The shape of the request, not its content: the sampling controls
        // and the layout a source is rendered into the window in. Two runs
        // that put the same sources in front of the same model in different
        // words are not one comparison
        // (`commonmeasure_inference::request_shape_digest`).
        "inference_request_shape": commonmeasure_inference::request_shape_digest(),
        "transport_timeout_seconds": commonmeasure_http::CLIENT_TIMEOUT.as_secs(),
        "retry_policy": "none; a failed acquisition is recorded, not retried",
        "cache_control": "not controlled. Providers may serve from their own caches: Exa \
                          returned source \"cached\" and Firecrawl cacheState \"hit\" during \
                          reconnaissance on 1 August 2026.",
    });
    if let (Value::Object(manifest), Value::Object(extra)) = (&mut manifest, extra) {
        manifest.extend(extra);
    }
    manifest
}

/// The job as a reader sees it, derived from the job's own constraint list
/// rather than restated, so what is displayed is necessarily what was enforced.
fn job_projection(suite: &Suite) -> Value {
    let job = &suite.job;
    json!({
        "id": job.id,
        "label": suite.label,
        "kind": job.kind,
        "prompt": job.prompt,
        "objective": job.objective,
        "constraints": {
            "enforcement_mode": job.policy_mode,
            "max_acquisition_cost": job.maximum_acquisition_cost(),
            "max_context_tokens": job.maximum_context_tokens(),
            "max_total_latency_ms": job.maximum_latency_ms(),
            "allowed_source_hosts": job.allowed_source_hosts().collect::<Vec<_>>(),
            "denied_source_hosts": job.denied_source_hosts().collect::<Vec<_>>(),
            "allowed_providers": job.allowed_providers().collect::<Vec<_>>(),
            "denied_providers": job.denied_providers().collect::<Vec<_>>(),
            "required_licences": job.required_licences().collect::<Vec<_>>(),
        },
        "requirements": job.evidence_requirements,
    })
}

/// A plan name reduced to something safe to be a filename.
///
/// A plan is named for its provider, and a skill provider carries a colon
/// (`skill:plugin-creator-only`). The colon is a legal character in a path
/// here and an illegal one elsewhere, and a published run that cannot be
/// checked out on another filesystem is not an inspectable artefact. The
/// plan keeps its own name — it names the provider, which is the honest
/// identity — and `response_ref` records where the bytes actually went, so
/// nothing is left implicit.
fn file_stem(plan: &str) -> String {
    plan.chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => character,
            _ => '-',
        })
        .collect()
}

fn supply_gap(error: &SupplyError) -> Gap {
    let reason = match error {
        SupplyError::CredentialMissing { .. }
        | SupplyError::Transport { .. }
        // A killed or oversized invocation is supply that did not arrive: the
        // skill was reached and produced nothing this run will use, which is
        // the same shape of gap as a provider that answered with nothing
        // usable. The detail carries which of the two it was.
        | SupplyError::Execution { .. }
        | SupplyError::Status { .. } => GapReason::ProviderUnavailable,
        SupplyError::Malformed { .. } => GapReason::EvidenceMissing,
        SupplyError::CapabilityUnavailable { .. } => GapReason::CapabilityUnavailable,
    };
    Gap::new(reason, error.to_string())
}

/// What the evidence log ended up holding.
#[derive(Debug, PartialEq, Eq)]
pub struct EvidenceOutcome {
    /// False when the log owes a gap it could not write. A run still publishes:
    /// the work happened and the artefacts describe it. What it must not do is
    /// publish claiming a complete record.
    pub complete: bool,
    pub records: u64,
    /// SHA-256 of the log as it stands. `None` when it could not be read back,
    /// which is itself a reason `complete` is false.
    pub digest: Option<String>,
}

/// Close the log: write the terminal record, give any owed gap a last chance to
/// land, then report what is actually on disk.
///
/// The order is the point. `run_completed` is appended *before* completeness is
/// read, because a terminal append that fails is exactly the case a summary
/// written earlier would misreport — and `record` deliberately does not
/// propagate that failure, so nothing downstream would notice.
///
/// If the terminal append failed, one more append is attempted: a pending gap is
/// materialised by the next successful write, so this is the log's last chance
/// to record its own hole rather than leaving the summary as the only place the
/// hole is mentioned.
pub fn finalise_log(log: &mut EvidenceLog, run_id: Uuid) -> EvidenceOutcome {
    record(log, "run_completed", json!({"run_id": run_id}));
    if log.has_pending_gap() {
        record(log, "evidence_incomplete", json!({"run_id": run_id}));
    }
    let read_back = std::fs::read(log.path()).ok();
    EvidenceOutcome {
        // Both conditions matter: an owed gap means a record is missing, and an
        // unreadable log means we cannot say what is there.
        complete: !log.has_pending_gap() && read_back.is_some(),
        records: read_back
            .as_deref()
            .map(|bytes| bytes.iter().filter(|byte| **byte == b'\n').count() as u64)
            .unwrap_or_default(),
        digest: read_back.as_deref().map(sha256_digest),
    }
}

/// Append to the evidence log.
///
/// A run does not abort because its log could not take a line: the work still
/// happened and the artefacts still describe it. What it must not do is finish
/// claiming a complete record, which is why `run.evidence_complete` reports the
/// pending gap. Remembering the failure belongs to the log, which is the only
/// place that knows whether the record landed.
fn record(log: &mut EvidenceLog, event: &str, payload: Value) {
    let _ = log.append(event, payload);
}

/// Load a suite from a JSON file.
pub fn load_suite(path: &Path) -> Result<Suite, RunError> {
    let encoded = std::fs::read(path)?;
    serde_json::from_slice(&encoded)
        .map_err(|error| RunError::Suite(format!("invalid suite {}: {error}", path.display())))
}
