//! The opt-in, manifest-bound `commonmeasure-sidecar/1` boundary.
//!
//! Declarations are not a socket or filesystem sandbox. Policy admits a
//! declared crossing; a digest pins the single script, not its dependencies.
//! Untrusted process output is never interpolated into errors or logs.

use std::io::{BufRead, BufReader, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use commonmeasure_types::canonical::{canonical_json, sha256_digest};
use commonmeasure_types::{Constraint, Gap, GapReason};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Answer, Case, Label, Model, ModelChecker, Question};

const PROTOCOL: &str = "commonmeasure-sidecar/1";
const MAX_LINE: u64 = 1024 * 1024;
const MAX_OUTPUT: usize = 8 * 1024 * 1024;
const MAX_CASES: usize = 1024;
const ENVIRONMENT: &[&str] = &["PATH", "HOME", "TMPDIR", "LANG", "UV_CACHE_DIR"];
// These are the questions the edge asks, not an attestation to a script's
// private prompt. The digest in the egress record has exactly this scope.
const QUESTIONS: &str = "supported: the passage supports everything in the sentence; contradicts: the passage states otherwise on a claim in the sentence";

type Result<T> = std::result::Result<T, &'static str>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    protocol: String,
    name: String,
    version: String,
    stage: String,
    capability: String,
    artefact: Artefact,
    runtime: Runtime,
    model: bool,
    network: Value,
    determinism: String,
    timeout_seconds: f64,
    questions: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Artefact {
    path: PathBuf,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Runtime {
    command: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Network {
    provider: String,
    origins: Vec<String>,
    credential_env: String,
}

struct Prepared {
    declaration: Manifest,
    network: Option<Network>,
    // Run the verified bytes, rather than reopen an operator-mutable path
    // after verification. The manifest describes one standalone script.
    script: Vec<u8>,
    timeout: Duration,
    identity: Value,
    admitting_term: Option<Value>,
}

fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 160
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_./:~+-".contains(&c))
}

fn origin(s: &str) -> bool {
    url::Url::parse(s).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https")
            && u.has_host()
            && u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && u.fragment().is_none()
            && u.path() == "/"
    })
}

fn positive(n: f64) -> bool {
    n.is_finite() && n > 0.0 && n <= 86400.0
}

fn probability(n: f64) -> bool {
    n.is_finite() && (0.0..=1.0).contains(&n)
}

fn prepare(path: &Path, constraints: &[Constraint], timeout: Option<f64>) -> Result<Prepared> {
    let bytes = std::fs::read(path).map_err(|_| "sidecar_manifest_unavailable")?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| "sidecar_manifest_invalid")?;
    let m: Manifest =
        serde_json::from_value(value.clone()).map_err(|_| "sidecar_manifest_invalid")?;
    if m.protocol != PROTOCOL {
        return Err("sidecar_protocol_unsupported");
    }
    if m.runtime.command != ["uv", "run", "--script"] {
        return Err("sidecar_runtime_unsupported");
    }
    if !identifier(&m.name)
        || !identifier(&m.version)
        || m.stage != "verify"
        || m.capability != "display-integrity-model"
        || !m.model
        || !positive(m.timeout_seconds)
        || !matches!(
            m.determinism.as_str(),
            "deterministic" | "seeded" | "non_deterministic"
        )
        || !(m.questions == ["supported"] || m.questions == ["supported", "contradicts"])
        || m.artefact.path.as_os_str().is_empty()
        || !m
            .artefact
            .path
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        || m.artefact.sha256.len() != 64
        || !m
            .artefact
            .sha256
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err("sidecar_manifest_invalid");
    }
    let network = if m.network == false {
        None
    } else {
        let n: Network =
            serde_json::from_value(m.network.clone()).map_err(|_| "sidecar_network_invalid")?;
        if !identifier(&n.provider)
            || n.origins.is_empty()
            || !n.origins.iter().all(|s| origin(s))
            || n.credential_env.is_empty()
            || !n.credential_env.bytes().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
            || n.credential_env.as_bytes()[0].is_ascii_digit()
            // A credential declaration must not enable Python/uv injection
            // through one of their own configuration variables.
            || n.credential_env.starts_with("PYTHON")
            || n.credential_env.starts_with("UV_")
            || n.credential_env.starts_with("LD_")
            || n.credential_env.starts_with("DYLD_")
            || matches!(n.credential_env.as_str(), "ENV" | "BASH_ENV")
            || ENVIRONMENT.contains(&n.credential_env.as_str())
        {
            return Err("sidecar_network_invalid");
        }
        Some(n)
    };
    let script_path = path
        .parent()
        .unwrap_or(Path::new("."))
        .join(&m.artefact.path);
    let script = std::fs::read(script_path).map_err(|_| "sidecar_artefact_unavailable")?;
    let artefact_digest = sha256_digest(&script);
    if artefact_digest != format!("sha256:{}", m.artefact.sha256) {
        return Err("artefact_digest_mismatch");
    }
    let mut identity = serde_json::to_value(super::manifest()).expect("manifest serialises");
    identity["implementation"] = json!(crate::processor::Implementation::Sidecar {
        artefact_digest,
        manifest_digest: sha256_digest(canonical_json(&value).as_bytes()),
        runtime: "uv run --script --offline",
        confinement: "declared",
    });
    identity["sidecar"] = value;
    identity["permissions"]["model"] = json!(true);
    identity["permissions"]["filesystem"] = json!(true);
    identity["permissions"]["network"] = json!(network.is_some());
    identity["permissions"]["credentials"] = json!(network.is_some());
    identity["determinism"] = json!(m.determinism);
    let seconds = match timeout {
        Some(n) if positive(n) => n.min(m.timeout_seconds),
        Some(_) => return Err("sidecar_timeout_invalid"),
        None => m.timeout_seconds,
    };
    identity["limits"] = json!({
        "hello_timeout_seconds": seconds, "batch_timeout_seconds": seconds,
        "kill": "whole_process_group", "max_line_bytes": MAX_LINE,
        "max_output_bytes": MAX_OUTPUT, "max_cases": MAX_CASES,
    });
    let admitting_term = network
        .as_ref()
        .and_then(|n| {
            constraints.iter().find(|term| {
                matches!(term, Constraint::ProcessorEgress { processor, provider }
            if processor == &m.name && provider == &n.provider)
            })
        })
        .map(|t| serde_json::to_value(t).expect("constraint serialises"));
    Ok(Prepared {
        declaration: m,
        network,
        script,
        timeout: Duration::from_secs_f64(seconds),
        identity,
        admitting_term,
    })
}

impl Prepared {
    fn admitted(&self) -> Result<()> {
        if let Some(n) = &self.network {
            if self.admitting_term.is_none() {
                return Err("processor_egress_refused");
            }
            if !std::env::var_os(&n.credential_env).is_some_and(|v| !v.is_empty()) {
                return Err("sidecar_credential_unset");
            }
        }
        if !cfg!(unix) {
            return Err("sidecar_process_groups_unavailable");
        }
        Ok(())
    }
}

/// One configured model invocation. Invalid configuration is retained as an
/// unavailable stage, never silently replaced with a successful model.
pub struct ConfiguredModel {
    checker: SidecarChecker,
    threshold: f64,
    temperature: f64,
    contradiction_threshold: f64,
    calibration: Value,
}

fn env_number(name: &str) -> Result<Option<f64>> {
    std::env::var_os(name)
        .map(|v| {
            v.to_str()
                .and_then(|s| s.parse().ok())
                .ok_or("sidecar_configuration_invalid")
        })
        .transpose()
}

fn calibration(p: &Prepared) -> Result<(f64, f64, Value)> {
    if let Some(path) = std::env::var_os("COMMONMEASURE_CHECKER_SUMMARY") {
        let bytes = std::fs::read(path).map_err(|_| "sidecar_summary_unavailable")?;
        let s: Value = serde_json::from_slice(&bytes).map_err(|_| "sidecar_summary_invalid")?;
        let all = &s["checkers"][&p.declaration.name]["per_set"]["all"];
        let target = env_number("COMMONMEASURE_CHECKER_TARGET")?.unwrap_or(0.05);
        if !probability(target) {
            return Err("sidecar_summary_invalid");
        }
        let temperature = all["temperature"]
            .as_f64()
            .filter(|n| n.is_finite() && *n > 0.0)
            .ok_or("sidecar_summary_invalid")?;
        let threshold = all["by_target"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|b| {
                b["target"]
                    .as_f64()
                    .is_some_and(|n| (n - target).abs() < 1e-9)
            })
            .and_then(|b| b["threshold"].as_f64())
            .filter(|n| probability(*n))
            .ok_or("sidecar_summary_threshold_missing")?;
        Ok((
            threshold,
            temperature,
            json!({"state": "calibrated", "summary_digest": sha256_digest(&bytes), "target": target}),
        ))
    } else {
        let threshold = env_number("COMMONMEASURE_CHECKER_THRESHOLD")?
            .filter(|n| probability(*n))
            .ok_or("sidecar_threshold_required")?;
        Ok((threshold, 1.0, json!({"state": "uncalibrated"})))
    }
}

/// Read operator configuration at tool invocation time. An unset variable
/// leaves the deterministic path and its record unchanged.
pub fn configured_model(constraints: &[Constraint]) -> Option<ConfiguredModel> {
    let path = PathBuf::from(std::env::var_os("COMMONMEASURE_CHECKER")?);
    let mut threshold = 1.0;
    let mut temperature = 1.0;
    let mut contradiction_threshold = 0.9;
    let mut evidence = json!({"state": "unavailable"});
    let prepared = env_number("COMMONMEASURE_CHECKER_TIMEOUT_SECONDS")
        .and_then(|timeout| prepare(&path, constraints, timeout))
        .and_then(|p| {
            (threshold, temperature, evidence) = calibration(&p)?;
            contradiction_threshold =
                env_number("COMMONMEASURE_CHECKER_CONTRADICTION_THRESHOLD")?.unwrap_or(0.9);
            if !probability(contradiction_threshold) {
                return Err("sidecar_configuration_invalid");
            }
            Ok(p)
        });
    Some(ConfiguredModel {
        checker: SidecarChecker {
            prepared,
            evidence: json!({"outcome": "not_run", "hello": null, "bye": null, "gaps": []}),
        },
        threshold,
        temperature,
        contradiction_threshold,
        calibration: evidence,
    })
}

impl ConfiguredModel {
    /// Render with this single-use model, retaining its complete lifecycle
    /// evidence before the caller appends the invocation.
    pub fn invoke(
        mut self,
        read: &crate::processor::release::Read,
        reference: &str,
        reply: &commonmeasure_types::display::Reply,
    ) -> (crate::processor::Invocation, Option<super::Rendered>) {
        let (mut invocation, rendered) = super::invoke_with(
            read,
            reference,
            reply,
            Some(Model {
                checker: &mut self.checker,
                threshold: self.threshold,
                temperature: self.temperature,
                contradiction_threshold: self.contradiction_threshold,
            }),
        );
        if let Some(model) = invocation.detail.get_mut("model").filter(|v| v.is_object()) {
            model["calibration"] = self.calibration;
            if model["egress"].is_object() {
                model["egress"]["threshold"] = json!(self.threshold);
            }
        }
        if let Ok(p) = &self.checker.prepared {
            let mut manifest = p.identity.clone();
            manifest["configuration_digest"] = json!(super::digest_of(&json!({
                "rules": super::manifest().configuration_digest,
                "sidecar": p.identity["implementation"], "threshold": self.threshold,
                "temperature": self.temperature, "contradiction_threshold": self.contradiction_threshold,
                "calibration": invocation.detail["model"]["calibration"],
            })));
            if invocation.detail["model"].is_object() {
                invocation.detail["model"]["manifest"] = manifest.clone();
            }
            invocation.configured_manifest = Some(manifest);
        }
        invocation
            .blind_spots
            .retain(|b| !b.starts_with("No model stage") && !b.starts_with("Confirmed means"));
        invocation.blind_spots.extend([
            "Model probabilities are the sidecar's declarations, not proof of truth or calibration.",
            "Network origins and filesystem access are declared, not confined; dependencies are not digest-pinned by this protocol.",
        ]);
        invocation = invocation.declared();
        for gap in self.checker.evidence["gaps"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            invocation.gaps.push(Gap::new(
                if matches!(gap, "sidecar_cost_unknown" | "sidecar_output_dropped") {
                    GapReason::EvidenceMissing
                } else {
                    GapReason::CapabilityUnavailable
                },
                gap,
            ));
        }
        (invocation, rendered)
    }
}

/// Inspect configuration and the last recorded invocation without spawning
/// a process. `reachable: null` means no successful invocation is known.
pub fn status(constraints: &[Constraint], last: Option<&Value>) -> Value {
    let Some(configured) = configured_model(constraints) else {
        return json!({"configured": false});
    };
    let path = std::env::var_os("COMMONMEASURE_CHECKER").map(PathBuf::from);
    let mut status = json!({"configured": true, "manifest_path": path, "confinement": "declared"});
    match &configured.checker.prepared {
        Ok(p) => {
            status["implementation"] = p.identity["implementation"].clone();
            status["network"] = p.declaration.network.clone();
            status["credential"] = p.network.as_ref().map(|n| json!({"variable": n.credential_env,
                "state": if std::env::var_os(&n.credential_env).is_some_and(|v| !v.is_empty()) { "set" } else { "unset" }
            })).unwrap_or(Value::Null);
            let last =
                last.filter(|l| l["manifest"]["implementation"] == p.identity["implementation"]);
            status["last_outcome"] = last.map(|l| l["outcome"].clone()).unwrap_or(Value::Null);
            status["hello"] = last.map(|l| l["hello"].clone()).unwrap_or(Value::Null);
            let gap = p.admitted().err();
            status["gap"] = gap
                .map(|g| json!(g))
                .or_else(|| last.map(|l| l["gaps"].clone()))
                .unwrap_or_else(|| json!("sidecar_not_invoked"));
            status["reachable"] = if gap.is_some() {
                json!(false)
            } else {
                last.map(|l| json!(l["outcome"] == "completed"))
                    .unwrap_or(Value::Null)
            };
        }
        Err(fault) => {
            status["gap"] = json!(fault);
            status["reachable"] = json!(false);
        }
    }
    status
}

struct SidecarChecker {
    prepared: Result<Prepared>,
    evidence: Value,
}

impl ModelChecker for SidecarChecker {
    fn name(&self) -> String {
        "sidecar-model/1".to_owned()
    }
    fn manifest(&self) -> Value {
        self.prepared
            .as_ref()
            .map(|p| p.identity.clone())
            .unwrap_or_else(|fault| json!({"gap": fault}))
    }
    fn questions(&self) -> Vec<Question> {
        let mut q = vec![Question::Supported];
        if self.prepared.as_ref().map_or(true, |p| {
            p.declaration.questions.iter().any(|q| q == "contradicts")
        }) {
            q.push(Question::Contradicts);
        }
        q
    }
    fn evidence(&self) -> Value {
        self.evidence.clone()
    }
    fn check_many(&mut self, cases: &[Case]) -> Vec<Vec<Answer>> {
        let mut gaps = Vec::new();
        let started = Instant::now();
        let mut hello = Value::Null;
        let mut bye = Value::Null;
        let mut hello_ms = None;
        let result = self.prepared.as_ref().map_err(|f| *f).and_then(|p| {
            p.admitted()?;
            run(p, cases, &mut hello, &mut bye, &mut hello_ms, &mut gaps)
        });
        let failed = result.is_err();
        let questions = self.questions();
        let answers: Vec<Vec<Answer>> = match result {
            Ok(replies) => replies
                .iter()
                .map(|reply| {
                    let fault = validate_reply(reply, questions.contains(&Question::Contradicts)).err();
                    if let Some(f) = fault {
                        gaps.push(f);
                    }
                    let detail = filter_detail(&reply["detail"], &mut gaps);
                    questions.iter().map(|q| {
                        let field = if *q == Question::Supported { "p_supported" } else { "p_contradicts" };
                        let probability = fault.is_none().then(|| reply[field].as_f64()).flatten();
                        Answer {
                            question: *q,
                            label: probability.map_or(Label::NotApplicable, |p| {
                                if p >= 0.5 { Label::Yes } else { Label::No }
                            }),
                            probability,
                            basis: self.name(),
                            reason: fault.unwrap_or("sidecar_probability_reported").to_owned(),
                            detail: json!({"reported": detail, "latency_ms": reply["latency_ms"].as_u64()}),
                        }
                    }).collect()
                })
                .collect(),
            Err(fault) => {
                gaps.push(fault);
                cases.iter().map(|_| {
                    questions.iter().map(|q| Answer {
                        question: *q, label: Label::NotApplicable, probability: None,
                        basis: self.name(), reason: fault.to_owned(), detail: Value::Null,
                    }).collect()
                }).collect()
            }
        };
        if self
            .prepared
            .as_ref()
            .is_ok_and(|p| p.network.is_some() && p.admitted().is_ok())
        {
            gaps.push("sidecar_cost_unknown");
        }
        gaps.sort_unstable();
        gaps.dedup();
        self.evidence = json!({"hello": hello, "bye": bye,
            "outcome": if failed || result_is_failed(&answers) { "failed" } else { "completed" },
            "gaps": gaps, "timings": {"hello_ms": hello_ms, "total_ms": started.elapsed().as_millis()},
        });
        if let Ok(p) = &self.prepared
            && let Some(n) = &p.network
        {
            self.evidence["egress"] = json!({
                "provider": n.provider, "origins": n.origins, "confinement": "declared",
                "admitting_term": p.admitting_term, "admitted": p.admitted().is_ok(),
                "model": self.evidence["hello"]["model"],
                "question_text_sha256": sha256_digest(QUESTIONS.as_bytes()),
                "threshold": Value::Null,
                "requests": cases.iter().map(|c| json!({
                    "request_sha256": super::digest_of(&request(c)),
                    "passage_sha256": sha256_digest(c.passage.as_bytes()),
                    "sentence_sha256": sha256_digest(c.summary.as_bytes()),
                })).collect::<Vec<_>>(), "case_count": cases.len(),
                "usage": self.evidence["bye"], "cost": Value::Null,
            });
        }
        answers
    }
}

fn result_is_failed(answers: &[Vec<Answer>]) -> bool {
    answers.iter().flatten().any(|a| a.probability.is_none())
}

fn request(case: &Case) -> Value {
    json!({"id": case.id, "passage": case.passage, "summary": case.summary})
}

fn validate_reply(reply: &Value, contradiction: bool) -> Result<()> {
    let Some(map) = reply.as_object() else {
        return Err("sidecar_answer_malformed");
    };
    if map.contains_key("error") {
        return Err("sidecar_item_error");
    }
    if !reply["p_supported"].as_f64().is_some_and(probability)
        || (contradiction && !reply["p_contradicts"].as_f64().is_some_and(probability))
        || (!reply["p_contradicts"].is_null()
            && !reply["p_contradicts"].as_f64().is_some_and(probability))
        || reply["latency_ms"].as_u64().is_none()
    {
        return Err("sidecar_answer_malformed");
    }
    Ok(())
}

// Only these shapes cross into the record. Unknown keys are not copied,
// including their names: a script can put the input into an object key too.
fn filter_detail(v: &Value, gaps: &mut Vec<&'static str>) -> Value {
    filter_detail_level(v, gaps, true)
}

fn filter_detail_level(v: &Value, gaps: &mut Vec<&'static str>, decisions: bool) -> Value {
    let Some(map) = v.as_object() else {
        if !v.is_null() {
            gaps.push("sidecar_output_dropped");
        }
        return json!({});
    };
    let array_limit = map.get("chunks").and_then(Value::as_u64).unwrap_or(64);
    let mut out = serde_json::Map::new();
    for (key, value) in map {
        let allowed = match key.as_str() {
            "p_raw"
            | "chunks"
            | "prompt_tokens"
            | "completion_tokens"
            | "input_tokens"
            | "output_tokens"
            | "latency_ms"
            | "threshold"
            | "temperature"
            | "contradiction_threshold"
            | "attempts"
            | "requests"
            | "errors"
            | "peak_memory_mb"
            | "peak_memory_bytes"
            | "tokens"
            | "probability_sum" => value.is_number() || value.is_null(),
            "p_per_chunk" => value
                .as_array()
                .is_some_and(|a| a.len() as u64 <= array_limit && a.iter().all(Value::is_number)),
            "bounded_letters" => value.as_array().is_some_and(|a| {
                a.iter()
                    .all(|x| matches!(x.as_str(), Some("A" | "B" | "C")))
            }),
            "probabilities" => value.as_object().is_some_and(|m| {
                m.iter().all(|(k, v)| {
                    matches!(
                        k.as_str(),
                        "supports" | "absent" | "contradicts" | "A" | "B" | "C"
                    ) && v.as_f64().is_some_and(probability)
                })
            }),
            "decisions" => {
                if let Some(a) = value
                    .as_array()
                    .filter(|a| decisions && a.len() as u64 <= array_limit)
                {
                    out.insert(
                        key.clone(),
                        Value::Array(
                            a.iter()
                                .map(|v| filter_detail_level(v, gaps, false))
                                .collect(),
                        ),
                    );
                } else {
                    gaps.push("sidecar_output_dropped");
                }
                continue;
            }
            _ => false,
        };
        if allowed {
            out.insert(key.clone(), value.clone());
        } else {
            gaps.push("sidecar_output_dropped");
        }
    }
    Value::Object(out)
}

fn filter_hello(v: &Value, gaps: &mut Vec<&'static str>) -> Value {
    let mut numeric = v.clone();
    if let Some(map) = numeric.as_object_mut() {
        for key in ["model", "family", "device", "endpoint", "questions"] {
            map.remove(key);
        }
    }
    let mut result = filter_detail(&numeric, gaps);
    for key in ["model", "family", "device"] {
        let Some(value) = v.get(key) else {
            continue;
        };
        if let Some(s) = value.as_str().filter(|s| {
            identifier(s)
                && if key == "device" {
                    s.len() <= 32 && !s.contains(['/', '~'])
                } else {
                    s.len() <= 64 && !s.starts_with(['/', '~', '.'])
                }
        }) {
            result[key] = json!(s);
        } else {
            gaps.push("sidecar_output_dropped");
        }
    }
    result["questions"] = v["questions"].clone();
    // Endpoint paths and queries are neither identity nor safe evidence.
    if let Some(endpoint) = v["endpoint"].as_str().and_then(|s| url::Url::parse(s).ok()) {
        result["endpoint_origin"] = json!(endpoint.origin().ascii_serialization());
    }
    result
}

struct Group(Child);
impl Drop for Group {
    fn drop(&mut self) {
        #[cfg(unix)]
        // SAFETY: this group was created by process_group(0) for our child.
        unsafe {
            libc::killpg(self.0.id() as libc::pid_t, libc::SIGKILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn receive(rx: &Receiver<Result<Value>>, deadline: Instant, fault: &'static str) -> Result<Value> {
    rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|e| {
            if e == mpsc::RecvTimeoutError::Timeout {
                fault
            } else {
                "sidecar_exited"
            }
        })?
}

fn run(
    p: &Prepared,
    cases: &[Case],
    hello: &mut Value,
    bye: &mut Value,
    hello_ms: &mut Option<u128>,
    gaps: &mut Vec<&'static str>,
) -> Result<Vec<Value>> {
    if cases.len() > MAX_CASES {
        return Err("sidecar_batch_too_large");
    }
    let snapshot = tempfile::tempdir().map_err(|_| "sidecar_start_failed")?;
    let script = snapshot.path().join("sidecar.py");
    std::fs::write(&script, &p.script).map_err(|_| "sidecar_start_failed")?;
    let mut command = Command::new("uv");
    command
        .args(["run", "--script", "--offline", "--no-config"])
        .arg(&script)
        .current_dir(snapshot.path())
        .env_clear();
    for key in ENVIRONMENT {
        if let Some(v) = std::env::var_os(key) {
            command.env(key, v);
        }
    }
    if let Some(n) = &p.network
        && let Some(v) = std::env::var_os(&n.credential_env)
    {
        command.env(&n.credential_env, v);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let started = Instant::now();
    let mut group = Group(
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // A sidecar may echo content to stderr. It must never inherit the
            // server's log stream, even on a uv dependency-resolution failure.
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "sidecar_start_failed")?,
    );
    let mut input = group.0.stdin.take().ok_or("sidecar_start_failed")?;
    let output = group.0.stdout.take().ok_or("sidecar_start_failed")?;
    let (tx, rx) = mpsc::channel();
    let max_lines = cases.len() + 3;
    std::thread::spawn(move || {
        let mut reader = BufReader::new(output);
        let mut total = 0;
        for _ in 0..max_lines {
            let mut line = Vec::new();
            use std::io::Read;
            let read = reader
                .by_ref()
                .take(MAX_LINE + 1)
                .read_until(b'\n', &mut line);
            if matches!(read, Ok(0)) {
                return;
            }
            total += line.len();
            let parsed = if read.is_err() || line.len() as u64 > MAX_LINE || total > MAX_OUTPUT {
                Err("sidecar_output_limit")
            } else if line.last() != Some(&b'\n') {
                Err("sidecar_protocol_malformed")
            } else {
                serde_json::from_slice(&line).map_err(|_| "sidecar_protocol_malformed")
            };
            let failed = parsed.is_err();
            if tx.send(parsed).is_err() || failed {
                return;
            }
        }
        let _ = tx.send(Err("sidecar_protocol_extra_output"));
    });
    let first = receive(&rx, started + p.timeout, "sidecar_hello_timeout").map_err(|f| {
        if f == "sidecar_exited" {
            "sidecar_start_failed"
        } else {
            f
        }
    })?;
    let raw = first
        .get("hello")
        .filter(|_| first.as_object().is_some_and(|m| m.len() == 1))
        .ok_or("sidecar_hello_invalid")?;
    if !raw.is_object() || raw["questions"] != json!(p.declaration.questions) {
        return Err("sidecar_hello_invalid");
    }
    *hello = filter_hello(raw, gaps);
    if hello.get("model").is_none() || hello.get("family").is_none() {
        gaps.push("sidecar_output_dropped");
        return Err("sidecar_hello_invalid");
    }
    *hello_ms = Some(started.elapsed().as_millis());
    let deadline = Instant::now() + p.timeout;
    let requests: Vec<_> = cases.iter().map(request).collect();
    let (written, write_result) = mpsc::channel();
    std::thread::spawn(move || {
        let result = requests
            .iter()
            .try_for_each(|r| writeln!(input, "{r}").map_err(|_| "sidecar_write_failed"));
        drop(input); // EOF elicits bye within this invocation.
        let _ = written.send(result);
    });
    let mut replies = Vec::new();
    for case in cases {
        let reply = receive(&rx, deadline, "sidecar_batch_timeout")?;
        if reply["id"] != case.id {
            return Err("sidecar_answer_order_invalid");
        }
        // Drop unknown top-level members, never keep their names or values.
        if reply.as_object().is_some_and(|m| {
            m.keys().any(|k| {
                ![
                    "id",
                    "p_supported",
                    "p_contradicts",
                    "error",
                    "latency_ms",
                    "detail",
                ]
                .contains(&k.as_str())
            })
        }) {
            gaps.push("sidecar_output_dropped");
        }
        replies.push(reply);
    }
    let last = receive(&rx, deadline, "sidecar_batch_timeout")?;
    let raw = last
        .get("bye")
        .filter(|v| v.is_object() && last.as_object().is_some_and(|m| m.len() == 1))
        .ok_or("sidecar_bye_invalid")?;
    *bye = filter_detail(raw, gaps);
    write_result
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| "sidecar_batch_timeout")??;
    match receive(&rx, deadline, "sidecar_batch_timeout") {
        Err("sidecar_exited") => (),
        Err(f) => return Err(f),
        Ok(_) => return Err("sidecar_protocol_extra_output"),
    }
    loop {
        if let Some(exit) = group.0.try_wait().map_err(|_| "sidecar_exit_failed")? {
            if !exit.success() {
                return Err("sidecar_exit_failed");
            }
            break;
        }
        if Instant::now() >= deadline {
            return Err("sidecar_batch_timeout");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(replies)
}

/// Check one manifest through the production supervisor with one fixed
/// synthetic case. Output has the same no-text filtering as session records.
/// No threshold is needed for this protocol check.
pub fn check(path: &Path, constraints: &[Constraint]) -> std::result::Result<Value, Value> {
    let prepared = env_number("COMMONMEASURE_CHECKER_TIMEOUT_SECONDS")
        .and_then(|timeout| prepare(path, constraints, timeout));
    let mut checker = SidecarChecker {
        prepared,
        evidence: json!({}),
    };
    let answers = checker.check_many(&[Case {
        id: "check-0".to_owned(),
        passage: "The bridge opened on Tuesday.".to_owned(),
        summary: "The bridge began operating on Tuesday.".to_owned(),
        markers: Vec::new(),
    }]);
    let report =
        json!({"manifest": checker.manifest(), "answers": answers, "lifecycle": checker.evidence});
    let dropped = report["lifecycle"]["gaps"]
        .as_array()
        .is_some_and(|gaps| gaps.iter().any(|gap| gap == "sidecar_output_dropped"));
    if report["lifecycle"]["outcome"] == "completed" && !dropped {
        Ok(report)
    } else {
        Err(report)
    }
}

#[cfg(test)]
mod output_filter_tests {
    use super::*;

    #[test]
    fn hello_identity_bounds_do_not_change_manifest_identifiers() {
        assert!(identifier(&"m".repeat(160)));
        assert!(!identifier(&"m".repeat(161)));
        for key in ["model", "family", "device"] {
            let limit = if key == "device" { 32 } else { 64 };
            for value in ["m".repeat(limit), "fixture/1".into(), "cpu:0".into()] {
                let mut gaps = Vec::new();
                let filtered = filter_hello(&json!({key: value}), &mut gaps);
                if key == "device" && value.contains('/') {
                    assert!(filtered.get(key).is_none());
                    assert_eq!(gaps, ["sidecar_output_dropped"]);
                } else {
                    assert_eq!(filtered[key], value);
                    assert!(gaps.is_empty());
                }
            }
            for value in [
                "m".repeat(limit + 1),
                "/cpu".into(),
                "~cpu".into(),
                "".into(),
                "two words".into(),
            ] {
                let mut gaps = Vec::new();
                assert!(
                    filter_hello(&json!({key: value}), &mut gaps)
                        .get(key)
                        .is_none()
                );
                assert_eq!(gaps, ["sidecar_output_dropped"]);
            }
        }
        for (key, value) in [
            ("model", ".hidden"),
            ("family", ".hidden"),
            ("device", "cpu/path"),
            ("device", "cpu~0"),
        ] {
            let mut gaps = Vec::new();
            assert!(
                filter_hello(&json!({key: value}), &mut gaps)
                    .get(key)
                    .is_none()
            );
            assert_eq!(gaps, ["sidecar_output_dropped"]);
        }
    }

    #[test]
    fn numeric_arrays_use_the_same_objects_integer_chunk_bound_or_default() {
        for key in ["p_per_chunk", "decisions"] {
            let element = if key == "decisions" {
                json!({"p_raw": 0.5})
            } else {
                json!(0.5)
            };
            for chunks in [
                None,
                Some(json!(-1)),
                Some(json!(1.5)),
                Some(Value::Null),
                Some(json!(0)),
                Some(json!(2)),
                Some(json!(u64::MAX)),
            ] {
                let limit = chunks.as_ref().and_then(Value::as_u64).unwrap_or(64);
                for length in [0, 1, 2, 3, 64, 65] {
                    let mut value = json!({key: vec![element.clone(); length]});
                    if let Some(chunks) = &chunks {
                        value["chunks"] = chunks.clone();
                    }
                    let mut gaps = Vec::new();
                    let filtered = filter_detail(&value, &mut gaps);
                    if length as u64 <= limit {
                        assert_eq!(filtered[key], value[key]);
                        assert!(gaps.is_empty());
                    } else {
                        assert!(filtered.get(key).is_none());
                        assert_eq!(gaps, ["sidecar_output_dropped"]);
                    }
                }
            }
        }
        let mut gaps = Vec::new();
        let filtered = filter_detail(&json!({"chunks": 0, "decisions": []}), &mut gaps);
        assert_eq!(filtered["decisions"], json!([]));
        assert!(gaps.is_empty());
        let filtered = filter_detail(
            &json!({"chunks": 1, "decisions": [{"chunks": 0, "p_per_chunk": [0.5]}]}),
            &mut gaps,
        );
        assert!(filtered["decisions"][0].get("p_per_chunk").is_none());
        assert_eq!(gaps, ["sidecar_output_dropped"]);
    }
}
