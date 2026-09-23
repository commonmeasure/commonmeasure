//! The run dossier: what ran, what it saw, what was real, and where the
//! evidence for each statement lives.
//!
//! `inspect` reads only the artefacts a run published — `summary.json`,
//! `manifest.json`, `evidence.ndjson` and, for replay runs, `replay.json` —
//! and prints one document a sceptical reader can check line by line. Every
//! claim ends with a citation naming the record it was read from, so the
//! command adds explanation without becoming a second truth store: nothing
//! is stated that the named files do not hold, and nothing is written back.
//!
//! Where the dossier can verify a claim instead of repeating it, it does:
//! the evidence log's digest is recomputed from the file on disk, the
//! manifest seal is recomputed from the `/manifest` object itself as well as
//! compared across artefacts, and a replay binding is checked by hashing the
//! sealed response bytes where they are present — a binding that does not
//! hash to its recording is reported as a mismatch rather than displayed as
//! provenance, and where `responses/` is not published the dossier says the
//! claim rests on the runtime's own record rather than presenting it as
//! verified. A supported citation's byte span gets the same treatment: the
//! retained text it points into is located in the sealed response by its
//! content hash and the span is excised and rechecked where the bytes are
//! present, and where they are not the dossier names where the text can be
//! obtained instead of stating a span the reader cannot resolve.

use std::fmt::Write as _;
use std::path::Path;

use commonmeasure_types::canonical::{canonical_digest, sha256_digest};
use serde_json::Value;

/// Wrap width for the whole document. Citations may extend past it rather
/// than wrap, because a citation split across lines cannot be copied whole.
const WIDTH: usize = 100;
/// The label column. Every line is `indent + label + body`, so the document
/// keeps one grid a reader can scan down.
const LABEL: usize = 13;

pub fn inspect(run: &Path) -> Result<(), String> {
    if run.is_file() {
        return crate::write_stdout(&session_dossier(run)?);
    }
    let artefacts = Artefacts::read(run);
    crate::write_stdout(&dossier(run, &artefacts?))
}

/// Read the private source record directly, preserving line references for the
/// handle joins. Invalid lines fail inspection rather than lowering counts.
fn session_dossier(path: &Path) -> Result<String, String> {
    let bytes =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let records: Vec<Value> = bytes
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let summary = commonmeasure_harness::session::host_observations(&records, None);
    let mut out = format!(
        "session source record: {}\n{}\nHost assertions do not establish semantic support or provider-internal context.\n",
        path.display(),
        summary.display()
    );
    for (line, record) in records.iter().enumerate() {
        let p = &record["payload"];
        if let Some(handle) = p["acquisition_id"]
            .as_str()
            .filter(|_| record["event"] == "crossing_mediated")
        {
            let observed =
                commonmeasure_harness::session::host_observations(&records, Some(handle));
            let _ = writeln!(
                out,
                "acquisition {handle}: {} [{}:{}]\n  {}",
                text(&p["url"]),
                path.display(),
                line + 1,
                observed.display()
            );
        } else if matches!(
            record["event"].as_str(),
            Some("context_entered" | "output_associated" | "observation_unavailable")
        ) {
            let _ = writeln!(
                out,
                "{} [{}:{}] {}",
                text(&record["event"]),
                path.display(),
                line + 1,
                p
            );
        }
    }
    Ok(out)
}

/// The published files, each either read or accounted for. Only the summary
/// is required: it is the contract (`docs/contracts/run-output.md`), and a
/// dossier without it would have nothing to cite.
struct Artefacts {
    summary: Value,
    manifest: Option<Value>,
    evidence: Option<EvidenceLog>,
    replay: Option<Value>,
    /// Plain-English accounts of what could not be read. Displayed, never
    /// swallowed: a missing artefact is a fact about the run directory.
    absences: Vec<String>,
}

struct EvidenceLog {
    /// `(seq, event, payload)` per parsed line, in file order. The seq stays
    /// optional: a record without one cannot be cited, and a stand-in number
    /// would name a line of the log that says something else.
    records: Vec<(Option<u64>, String, Value)>,
    /// SHA-256 of the raw file, computed here so the summary's claim about
    /// the log can be checked rather than repeated.
    digest: String,
}

impl Artefacts {
    fn read(run: &Path) -> Result<Self, String> {
        let summary_path = run.join("summary.json");
        let summary: Value = serde_json::from_slice(
            &std::fs::read(&summary_path)
                .map_err(|error| format!("cannot read {}: {error}", summary_path.display()))?,
        )
        .map_err(|error| format!("summary.json is not valid JSON: {error}"))?;

        let mut absences = Vec::new();
        let manifest = match std::fs::read(run.join("manifest.json")) {
            // A file that exists but does not parse is treated exactly like a
            // file that could not be read: a corrupt artefact must be at
            // least as loud as a deleted one.
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(value) => Some(value),
                Err(error) => {
                    absences.push(format!(
                        "manifest.json exists but is not valid JSON ({error}); the summary's \
                         manifest hash stands uncorroborated"
                    ));
                    None
                }
            },
            Err(error) => {
                absences.push(format!(
                    "manifest.json could not be read ({error}); the summary's manifest hash \
                     stands uncorroborated"
                ));
                None
            }
        };
        let evidence = match std::fs::read(run.join("evidence.ndjson")) {
            Ok(bytes) => {
                let mut unparseable = 0usize;
                let records = bytes
                    .split(|byte| *byte == b'\n')
                    .filter(|line| !line.is_empty())
                    .filter_map(|line| match serde_json::from_slice::<Value>(line) {
                        Ok(record) => Some(record),
                        Err(_) => {
                            unparseable += 1;
                            None
                        }
                    })
                    .map(|record| {
                        (
                            record["seq"].as_u64(),
                            record["event"].as_str().unwrap_or_default().to_owned(),
                            record["payload"].clone(),
                        )
                    })
                    .collect();
                if unparseable > 0 {
                    absences.push(format!(
                        "evidence.ndjson holds {unparseable} line{} that {} not valid JSON; \
                         the dropped records cannot corroborate the summary",
                        if unparseable == 1 { "" } else { "s" },
                        if unparseable == 1 { "is" } else { "are" },
                    ));
                }
                Some(EvidenceLog {
                    records,
                    digest: sha256_digest(&bytes),
                })
            }
            Err(error) => {
                absences.push(format!(
                    "evidence.ndjson could not be read ({error}); e: citations are omitted and \
                     the log cannot corroborate the summary"
                ));
                None
            }
        };
        let replay = match std::fs::read(run.join("replay.json")) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(value) => Some(value),
                Err(error) => {
                    absences.push(format!(
                        "replay.json exists but is not valid JSON ({error}); the per-plan \
                         replay records below are the remaining binding"
                    ));
                    None
                }
            },
            Err(error) => {
                if summary["run"]["mode"] == "replay" {
                    absences.push(format!(
                        "replay.json could not be read ({error}), yet the run claims replay \
                         mode; the per-plan replay records below are the remaining binding"
                    ));
                }
                None
            }
        };
        Ok(Self {
            summary,
            manifest,
            evidence,
            replay,
            absences,
        })
    }
}

impl EvidenceLog {
    /// The seq of the `plan_completed` record for a plan id.
    fn plan_seq(&self, plan_id: &str) -> Option<u64> {
        self.records
            .iter()
            .find(|(_, event, payload)| event == "plan_completed" && payload["id"] == plan_id)
            .and_then(|(seq, _, _)| *seq)
    }

    /// The seq of the `processor_invoked` record equal to this invocation.
    /// The summary embeds the identical payload in the plan, so equality is
    /// the join key; there is no separate identifier to drift.
    fn processor_seq(&self, invocation: &Value) -> Option<u64> {
        self.records
            .iter()
            .find(|(_, event, payload)| event == "processor_invoked" && payload == invocation)
            .and_then(|(seq, _, _)| *seq)
    }

    fn last_event(&self) -> Option<&str> {
        self.records.last().map(|(_, event, _)| event.as_str())
    }
}

/// One line of the dossier: `indent + label + wrapped body + [citations]`.
///
/// The citation never wraps. It is the part a reader copies into `jq`, and a
/// pointer split across lines cannot be copied whole, so a citation that does
/// not fit moves to its own line instead.
fn entry(out: &mut String, indent: usize, label: &str, body: &str, cites: &[String]) {
    let hang = indent + LABEL;
    let mut line = format!("{:indent$}{label:<LABEL$}", "", indent = indent);
    // Guard against a label wider than the column: keep one space before the
    // body rather than letting the words fuse.
    if !line.ends_with(' ') {
        line.push(' ');
    }
    let mut first_word = true;
    for word in body.split_whitespace() {
        if !first_word && line.len() + 1 + word.len() > WIDTH {
            out.push_str(line.trim_end());
            out.push('\n');
            line = format!("{:hang$}", "");
        } else if !first_word {
            line.push(' ');
        }
        line.push_str(word);
        first_word = false;
    }
    if !cites.is_empty() {
        let cite = format!("[{}]", cites.join("; "));
        if line.len() + 2 + cite.len() > WIDTH + 10 {
            out.push_str(line.trim_end());
            out.push('\n');
            line = format!("{:hang$}{cite}", "");
        } else {
            let _ = write!(line, "  {cite}");
        }
    }
    out.push_str(line.trim_end());
    out.push('\n');
}

pub(crate) fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "unknown".to_owned(),
        other => other.to_string(),
    }
}

/// A content hash short enough to sit inside a sentence. The full value is
/// one citation away; sealed response hashes, which a reader recomputes with
/// `sha256sum`, are always printed whole.
fn short_hash(hash: &str) -> String {
    match hash.strip_prefix("sha256:") {
        Some(hex) if hex.len() > 16 => format!("sha256:{}..{}", &hex[..8], &hex[hex.len() - 4..]),
        _ => hash.to_owned(),
    }
}

fn dossier(run: &Path, artefacts: &Artefacts) -> String {
    let summary = &artefacts.summary;
    let mut out = String::new();

    let mut read_from = vec!["summary.json"];
    if artefacts.manifest.is_some() {
        read_from.push("manifest.json");
    }
    if artefacts.evidence.is_some() {
        read_from.push("evidence.ndjson");
    }
    if artefacts.replay.is_some() {
        read_from.push("replay.json");
    }
    entry(&mut out, 0, "run dossier", &run.display().to_string(), &[]);
    entry(
        &mut out,
        0,
        "read from",
        &format!(
            "{} — nothing else is consulted, and nothing new is written",
            read_from.join(", ")
        ),
        &[],
    );
    // Which contract shape the reader is looking at, stated rather than
    // assumed: an artefact from an earlier contract renders fine, but it must
    // not pass silently as the current one.
    let contract = match summary["schema_version"].as_str() {
        Some(version) if version == commonmeasure_runtime::SCHEMA_VERSION => {
            format!("{version} — the current run contract (docs/contracts/run-output.md)")
        }
        Some(version) => format!(
            "{version} — NOT the current contract ({}); this run predates it and is read as \
             it declares itself",
            commonmeasure_runtime::SCHEMA_VERSION
        ),
        None => format!(
            "summary.json declares no schema_version; the current contract is {}",
            commonmeasure_runtime::SCHEMA_VERSION
        ),
    };
    let contract_cites: Vec<String> = cite_at('s', summary, "/schema_version")
        .into_iter()
        .collect();
    entry(&mut out, 0, "contract", &contract, &contract_cites);
    entry(
        &mut out,
        0,
        "citations",
        "every claim ends with the record it was read from: s:<pointer> resolves in \
         summary.json, e:<n> is the evidence.ndjson line whose seq is n, r:<pointer> resolves \
         in replay.json, m:<pointer> in manifest.json. Resolve a pointer with jq: \
         s:/plans/1/acquisition is jq '.plans[1].acquisition' summary.json",
        &[],
    );
    for absence in &artefacts.absences {
        entry(&mut out, 0, "unreadable", absence, &[]);
    }
    out.push('\n');

    entry(
        &mut out,
        0,
        "run",
        &format!(
            "{}, started {}",
            text(&summary["run"]["id"]),
            text(&summary["run"]["started_at"])
        ),
        &[cite_s("/run")],
    );
    entry(
        &mut out,
        0,
        "suite",
        &text(&summary["suite_version"]),
        &[cite_s("/suite_version")],
    );
    entry(
        &mut out,
        0,
        "job",
        &format!(
            "{} ({})",
            text(&summary["job"]["label"]),
            text(&summary["job"]["kind"])
        ),
        &[cite_s("/job")],
    );
    let mode = text(&summary["run"]["mode"]);
    entry(
        &mut out,
        0,
        "mode",
        &format!("{mode} — {}", mode_gloss(&mode)),
        &[cite_s("/run/mode")],
    );
    manifest_line(&mut out, summary, artefacts.manifest.as_ref());
    evidence_line(&mut out, summary, artefacts.evidence.as_ref());
    let basis = text(&summary["run"]["token_basis"]);
    entry(
        &mut out,
        0,
        "token basis",
        &format!("{basis} — {}", token_basis_gloss(&basis)),
        &[cite_s("/run/token_basis")],
    );
    out.push('\n');

    entry(
        &mut out,
        0,
        "question",
        &format!("\"{}\"", text(&summary["job"]["prompt"])),
        &[cite_s("/job/prompt")],
    );
    policy_block(&mut out, &summary["job"]);
    model_line(&mut out, &summary["model_plan"]);
    if let Some(replay) = &artefacts.replay {
        replay_line(&mut out, run, summary, replay);
    }
    out.push('\n');

    for (index, plan) in summary["plans"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        plan_section(
            &mut out,
            run,
            index,
            plan,
            artefacts.evidence.as_ref(),
            artefacts.replay.as_ref(),
        );
        out.push('\n');
    }

    selection_block(&mut out, summary);
    out
}

fn cite_s(pointer: &str) -> String {
    format!("s:{pointer}")
}

fn cite_e(seq: Option<u64>) -> Option<String> {
    seq.map(|seq| format!("e:{seq}"))
}

/// A citation emitted only where the pointer resolves in the artefact it
/// names. A citation to a record that is not there is invented provenance,
/// which is the one thing this document may never print.
fn cite_at(prefix: char, artefact: &Value, pointer: &str) -> Option<String> {
    artefact
        .pointer(pointer)
        .filter(|value| !value.is_null())
        .map(|_| format!("{prefix}:{pointer}"))
}

fn mode_gloss(mode: &str) -> String {
    match mode {
        "live" => "external providers were called for real and any catalogued skill was \
                   really executed; sealed responses are live captures, observed charges are \
                   real money, and a produced result is what a program on this machine \
                   printed"
            .to_owned(),
        "no-external-acquisition" => "this run was not authorised to call external providers \
                                      or to execute anything, so no provider acquisition was \
                                      attempted and no skill was invoked; only supply the \
                                      operator already owns could run"
            .to_owned(),
        "replay" => "provider responses were served from verified recordings over loopback, \
                     and no external provider was contacted. Inference is never replayed: an \
                     answer can only come from a real gateway call at run time"
            .to_owned(),
        other => format!("an undocumented mode ({other}); read docs/contracts/run-output.md"),
    }
}

fn token_basis_gloss(basis: &str) -> String {
    match basis {
        "whitespace-words" => {
            "every token count below is a word count, not a model tokenisation".to_owned()
        }
        other => format!(
            "every token count below is counted on the {other} basis; read \
             docs/contracts/run-output.md"
        ),
    }
}

fn manifest_line(out: &mut String, summary: &Value, manifest: Option<&Value>) {
    // Agreement is claimed only where both artefacts actually hold a hash.
    // Comparing the two JSON values would read a key missing from each side as
    // null, and report that mutual absence as corroboration.
    let claimed = summary["run"]["manifest_hash"].as_str();
    let sealed = manifest.and_then(|manifest| manifest["hash"].as_str());
    // The seal itself is recomputed from the `/manifest` object, the same
    // SHA-256 over the canonical serialisation the runtime takes when it
    // writes the file. Comparing `/hash` against the summary alone compares
    // two claims: a manifest whose sealed content was edited under an
    // untouched hash would read as corroborated.
    let recomputed = manifest
        .and_then(|manifest| manifest.pointer("/manifest"))
        .filter(|object| object.is_object())
        .map(canonical_digest);
    let corroboration = match (claimed, sealed) {
        (Some(claimed), Some(sealed)) if claimed == sealed => {
            "manifest.json carries the identical hash".to_owned()
        }
        (Some(_), Some(sealed)) => format!(
            "MISMATCH: manifest.json says {sealed}, so one of the two artefacts is not from \
             this run"
        ),
        (None, Some(sealed)) => format!(
            "manifest.json seals itself as {sealed}, which the summary neither repeats nor \
             contradicts"
        ),
        (_, None) if manifest.is_none() => "no valid manifest.json was read, so nothing \
             corroborates this run's seal (the unreadable note above says why)"
            .to_owned(),
        (_, None) => "manifest.json has no hash present at /hash, so nothing corroborates \
             this run's seal"
            .to_owned(),
    };
    let seal_check = match (sealed, recomputed.as_deref()) {
        (Some(sealed), Some(recomputed)) if sealed == recomputed => {
            "; the /manifest object hashes to that seal, recomputed here".to_owned()
        }
        (Some(_), Some(recomputed)) => format!(
            "; SEAL MISMATCH: the /manifest object hashes to {recomputed}, recomputed here — \
             the hash manifest.json carries does not seal this content"
        ),
        (Some(_), None) => "; the seal could not be recomputed here because manifest.json \
             holds no /manifest object to hash"
            .to_owned(),
        (None, _) => String::new(),
    };
    let corroboration = format!("{corroboration}{seal_check}");
    let inventory = match manifest.and_then(sealed_inventory) {
        Some(keys) => format!("every key of manifest.json's /manifest object: {keys}"),
        None => "the inputs manifest.json's /manifest object names, which could not be read \
                 here (docs/contracts/run-output.md, Manifest)"
            .to_owned(),
    };
    let body = match claimed {
        Some(claimed) => format!("{claimed} — seals {inventory}; {corroboration}"),
        None => format!(
            "no hash present — summary.json states no manifest hash, so nothing here attests \
             to {inventory}; {corroboration}"
        ),
    };
    let mut cites: Vec<String> = cite_at('s', summary, "/run/manifest_hash")
        .into_iter()
        .collect();
    if let Some(manifest) = manifest {
        cites.extend(cite_at('m', manifest, "/hash"));
        cites.extend(cite_at('m', manifest, "/manifest"));
    }
    entry(out, 0, "manifest", &body, &cites);
}

/// What the manifest hash seals, read from the manifest rather than restated
/// here. The runtime hashes the `/manifest` object whole, so its key set *is*
/// the sealed inventory: a v1 and a v2 artefact each describe themselves, and
/// a newly sealed input appears the run it is added, with no edit here.
fn sealed_inventory(manifest: &Value) -> Option<String> {
    let sealed = manifest.pointer("/manifest")?.as_object()?;
    if sealed.is_empty() {
        return None;
    }
    // Presentation only: the keys a reader meets most often lead, in the
    // order `docs/contracts/run-output.md` explains them. Anything absent is
    // skipped and anything unlisted still prints, in the manifest's own
    // order, so this list can never make the line say something untrue.
    const READING_ORDER: [&str; 16] = [
        "manifest_version",
        "suite_version",
        "label",
        "job",
        "supply_plans",
        "model_plan",
        "result_limit",
        "system_prompt",
        "adapter_version",
        "processors",
        "evaluators",
        "inference_gateway",
        "token_basis",
        "transport_timeout_seconds",
        "retry_policy",
        "cache_control",
    ];
    let mut keys: Vec<&str> = READING_ORDER
        .iter()
        .copied()
        .filter(|key| sealed.contains_key(*key))
        .collect();
    keys.extend(
        sealed
            .keys()
            .map(String::as_str)
            .filter(|key| !READING_ORDER.contains(key)),
    );
    Some(keys.join(", "))
}

fn evidence_line(out: &mut String, summary: &Value, evidence: Option<&EvidenceLog>) {
    let complete = summary["run"]["evidence_complete"] == Value::Bool(true);
    let claimed_records = text(&summary["evidence_log"]["records"]);
    let claimed_digest = text(&summary["evidence_log"]["sha256"]);
    let mut body = if complete {
        format!("complete, {claimed_records} records")
    } else {
        format!(
            "INCOMPLETE, {claimed_records} records — the log owes at least one record it \
             could not write; look for evidence_gap lines"
        )
    };
    let mut cites: Vec<String> = ["/run/evidence_complete", "/evidence_log"]
        .iter()
        .filter_map(|pointer| cite_at('s', summary, pointer))
        .collect();
    match evidence {
        Some(log) => {
            if log.digest == claimed_digest {
                let _ = write!(
                    body,
                    "; the log on disk hashes to the summary's claim ({}), recomputed here",
                    short_hash(&claimed_digest)
                );
            } else {
                let _ = write!(
                    body,
                    "; DIGEST MISMATCH: the summary claims {claimed_digest} but the file \
                     hashes to {} — the log and summary are not from the same run",
                    log.digest
                );
            }
            match log.last_event() {
                Some("run_completed") => {
                    let _ = write!(body, "; the final record is run_completed");
                    if let Some((seq, _, _)) = log.records.last() {
                        cites.extend(cite_e(*seq));
                    }
                }
                Some(other) => {
                    let _ = write!(
                        body,
                        "; the final record is {other}, not run_completed — the run did not \
                         close its log normally"
                    );
                }
                None => {
                    let _ = write!(body, "; the log holds no parseable records");
                }
            }
        }
        None => {
            let _ = write!(body, "; the log itself could not be read back here");
        }
    }
    entry(out, 0, "evidence", &body, &cites);
}

fn policy_block(out: &mut String, job: &Value) {
    let constraints = &job["constraints"];
    let mut declared = Vec::new();
    if let Some(cost) = constraints["max_acquisition_cost"].as_object() {
        declared.push(format!(
            "spend at most {} {} per acquisition",
            cost.get("micros")
                .and_then(Value::as_f64)
                .map(|micros| format!("{}", micros / 1_000_000.0))
                .unwrap_or_else(|| "?".to_owned()),
            cost.get("currency").map(text).unwrap_or_default()
        ));
    }
    if let Some(tokens) = constraints["max_context_tokens"].as_u64() {
        declared.push(format!("admit at most {tokens} tokens of context"));
    }
    match constraints["max_total_latency_ms"].as_u64() {
        Some(ms) => declared.push(format!("finish each plan within {ms} ms")),
        None => declared.push("no latency cap".to_owned()),
    }
    let mut lists = Vec::new();
    for (key, phrase) in [
        ("allowed_providers", "providers allowed"),
        ("denied_providers", "providers denied"),
        ("allowed_source_hosts", "source hosts allowed"),
        ("denied_source_hosts", "source hosts denied"),
    ] {
        let names: Vec<String> = constraints[key]
            .as_array()
            .into_iter()
            .flatten()
            .map(text)
            .collect();
        if !names.is_empty() {
            lists.push(format!("{phrase}: {}", names.join(", ")));
        }
    }
    if lists.is_empty() {
        declared.push("no provider or source-host allow/deny lists".to_owned());
    } else {
        declared.extend(lists);
    }
    let licences: Vec<String> = constraints["required_licences"]
        .as_array()
        .into_iter()
        .flatten()
        .map(text)
        .collect();
    if licences.is_empty() {
        declared.push("no licence requirement".to_owned());
    } else {
        declared.push(format!("required licences: {}", licences.join(", ")));
    }
    declared.push(format!(
        "enforcement {}",
        text(&constraints["enforcement_mode"])
    ));
    entry(
        out,
        0,
        "declared",
        &format!("{}.", declared.join("; ")),
        &[cite_s("/job/constraints")],
    );
    for (index, requirement) in job["requirements"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        entry(
            out,
            0,
            "requires",
            &format!(
                "{}{}",
                text(&requirement["description"]),
                if requirement["required"] == Value::Bool(true) {
                    " (required)"
                } else {
                    " (preferred)"
                }
            ),
            &[cite_s(&format!("/job/requirements/{index}"))],
        );
    }
    if let Some(objective) = job["objective"].as_object() {
        let weights: Vec<String> = objective
            .iter()
            .filter(|(key, _)| *key != "kind")
            .map(|(key, weight)| format!("{key} {}", text(weight)))
            .collect();
        entry(
            out,
            0,
            "objective",
            &format!(
                "{} — {}",
                text(&job["objective"]["kind"]),
                weights.join(", ")
            ),
            &[cite_s("/job/objective")],
        );
    }
}

fn model_line(out: &mut String, model_plan: &Value) {
    let body = if model_plan["configured"] == Value::Bool(true) {
        format!(
            "{} via the {} gateway at {}",
            text(&model_plan["requested_model"]),
            text(&model_plan["gateway"]),
            text(&model_plan["endpoint"])
        )
    } else {
        format!(
            "no inference gateway is configured, so no plan could reach a model; the suite \
             would have requested {}",
            text(&model_plan["requested_model"])
        )
    };
    entry(out, 0, "model", &body, &[cite_s("/model_plan")]);
}

/// SHA-256 of a sealed response file, or None when the bytes are not present
/// in the run directory. `responses/` is deliberately unpublished for most
/// runs — sealed provider bodies may carry licensed content — so absence is a
/// normal state the dossier reports as one, never an error.
fn response_hash(run: &Path, reference: &str) -> Option<String> {
    let bytes = std::fs::read(run.join(reference)).ok()?;
    Some(sha256_digest(&bytes))
}

fn replay_line(out: &mut String, run: &Path, summary: &Value, replay: &Value) {
    let bindings = replay["bindings"].as_array().cloned().unwrap_or_default();
    // Each binding is checked against the bytes where they exist: the sealed
    // response file is hashed here and compared to the recording's hash. The
    // runtime's `matches_recorded_input` boolean is its own claim about the
    // same comparison, and it carries the verdict only where the bytes are
    // not present to recheck — which the dossier then says plainly.
    let mut verified = 0usize;
    let mut mismatched: Vec<String> = Vec::new();
    let mut confessed: Vec<String> = Vec::new();
    let mut unverified: Vec<String> = Vec::new();
    let mut unanchored: Vec<String> = Vec::new();
    for binding in &bindings {
        let plan = text(&binding["plan"]);
        let recomputed = binding["sealed_response_ref"]
            .as_str()
            .and_then(|reference| response_hash(run, reference));
        match (recomputed, binding["recorded_response_sha256"].as_str()) {
            (Some(actual), Some(recorded)) if actual == recorded => verified += 1,
            (Some(_), Some(_)) => mismatched.push(plan),
            (_, None) => unanchored.push(plan),
            (None, Some(_)) if binding["matches_recorded_input"] == Value::Bool(true) => {
                unverified.push(plan)
            }
            (None, Some(_)) => confessed.push(plan),
        }
    }
    let mut clauses: Vec<String> = Vec::new();
    if bindings.is_empty() {
        clauses.push("no plan bindings are recorded".to_owned());
    }
    if !mismatched.is_empty() {
        clauses.push(format!(
            "MISMATCH on {} — the sealed bytes do not hash to the recording they claim, \
             recomputed here",
            mismatched.join(", ")
        ));
    }
    if !confessed.is_empty() {
        clauses.push(format!(
            "MISMATCH on {} — the runtime's own record says those sealed bytes are not the \
             recording they claim, and the bytes are not present here to recheck",
            confessed.join(", ")
        ));
    }
    if verified > 0 {
        clauses.push(
            if mismatched.is_empty()
                && confessed.is_empty()
                && unverified.is_empty()
                && unanchored.is_empty()
            {
                format!(
                    "every sealed response hash equals its recording's ({verified} plan \
                     bindings), recomputed here from the sealed bytes"
                )
            } else {
                format!("{verified} sealed response(s) hash to their recordings, recomputed here")
            },
        );
    }
    if !unverified.is_empty() {
        clauses.push(format!(
            "the sealed response bytes for {} are not present in this directory, so their \
             recorded-hash claim rests on the runtime's own matches_recorded_input record and \
             is not verified here",
            unverified.join(", ")
        ));
    }
    if !unanchored.is_empty() {
        clauses.push(format!(
            "no recorded hash is present for {} to recheck against",
            unanchored.join(", ")
        ));
    }
    let verdict = clauses.join("; ");
    entry(
        out,
        0,
        "recordings",
        &format!(
            "{} recorded responses verified against {}; {verdict}",
            replay["recordings"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            text(&summary["run"]["replay_manifest"]),
        ),
        &[
            cite_s("/run/replay_manifest"),
            "r:/recordings".to_owned(),
            "r:/bindings".to_owned(),
        ],
    );
}

fn plan_section(
    out: &mut String,
    run: &Path,
    index: usize,
    plan: &Value,
    evidence: Option<&EvidenceLog>,
    replay: Option<&Value>,
) {
    let p = |suffix: &str| cite_s(&format!("/plans/{index}{suffix}"));
    let plan_id = text(&plan["id"]);
    let plan_seq = evidence.and_then(|log| log.plan_seq(&plan_id));

    let status = text(&plan["status"]);
    let gloss = match status.as_str() {
        "completed" if plan["provider"] == "none" => {
            "the controlled baseline — the model answered with no external context, so the \
             other answers can be measured against it"
        }
        "completed" => "it ran to an answer",
        "refused" => "policy refused it before execution",
        "unavailable" => {
            "it could not produce an answer; each gap below names a missing \
                          dependency in plain terms"
        }
        _ => "an undocumented status",
    };
    let mut header_cites = vec![p("")];
    header_cites.extend(cite_e(plan_seq));
    entry(
        out,
        0,
        "plan",
        &format!("{plan_id} — {status}: {gloss}"),
        &header_cites,
    );

    if plan["provider"] != "none" {
        entry(
            out,
            2,
            "supply",
            &format!(
                "{} ({}), adapter {}",
                text(&plan["provider"]),
                text(&plan["capability"]),
                text(&plan["adapter_version"])
            ),
            &[p("/provider"), p("/capability"), p("/adapter_version")],
        );
    }
    let state = text(&plan["verification_state"]);
    entry(
        out,
        2,
        "verified",
        &format!(
            "{state} — {}",
            verification_gloss(&state, &text(&plan["capability"]))
        ),
        &[p("/verification_state")],
    );
    let eligibility = &plan["eligibility"];
    entry(
        out,
        2,
        "eligibility",
        &format!(
            "{} — {}",
            if eligibility["eligible"] == Value::Bool(true) {
                "eligible"
            } else {
                "not eligible"
            },
            text(&eligibility["reason"])
        ),
        &[p("/eligibility")],
    );

    bytes_lines(out, run, index, plan, replay, &plan_id);
    policy_lines(out, index, plan);
    source_lines(out, index, plan);
    processor_lines(out, index, plan, evidence);
    inference_lines(out, index, plan);

    match plan["answer"].as_str() {
        Some(answer) => entry(out, 2, "answer", &format!("\"{answer}\""), &[p("/answer")]),
        None => entry(
            out,
            2,
            "answer",
            "none — no inference completed for this plan, so there is nothing to quote",
            &[p("/answer")],
        ),
    }

    evaluation_lines(out, run, index, plan);
    fidelity_lines(out, index, plan, evidence);
    provenance_lines(out, index, plan, evidence);

    let gaps = plan["gaps"].as_array().cloned().unwrap_or_default();
    if gaps.is_empty() {
        entry(
            out,
            2,
            "gaps",
            "none — nothing this plan needed went unobtained",
            &[p("/gaps")],
        );
    }
    for (gap_index, gap) in gaps.iter().enumerate() {
        entry(
            out,
            2,
            "gap",
            &format!(
                "{} — {}",
                gap_reason_gloss(&text(&gap["reason"])),
                text(&gap["detail"])
            ),
            &[p(&format!("/gaps/{gap_index}"))],
        );
    }
}

/// The per-plan evaluation sections. A v4 summary nests one section per
/// evaluator — grounding, coverage, freshness — while earlier runs carry the
/// grounding record directly under `evaluation`; both are read as they
/// declare themselves, and the pointer each citation prints is the pointer
/// that resolves in that artefact.
fn evaluation_lines(out: &mut String, run: &Path, index: usize, plan: &Value) {
    let p = |pointer: &str| cite_s(&format!("/plans/{index}{pointer}"));
    let evaluation = &plan["evaluation"];
    if evaluation.is_null() {
        // A genuinely pre-evaluator summary has no `evaluation` key at all —
        // the key entered the contract with the evaluator — and a citation to
        // a pointer that resolves in no artefact is invented provenance. The
        // explicit-null shape is cited; the absent key is cited by nothing,
        // because absence has no record to cite.
        let cites: Vec<String> = plan
            .as_object()
            .is_some_and(|plan| plan.contains_key("evaluation"))
            .then(|| p("/evaluation"))
            .into_iter()
            .collect();
        entry(
            out,
            2,
            "grounding",
            "no record — this run predates the grounding evaluator, which publishes no \
             measurement rather than inventing one after the fact",
            &cites,
        );
        return;
    }
    let (grounding, base) = match evaluation.get("grounding") {
        Some(section) if section.is_object() => (section, "/evaluation/grounding"),
        _ => (evaluation, "/evaluation"),
    };
    grounding_lines(out, run, index, plan, grounding, base);
    match evaluation.get("coverage").filter(|value| value.is_object()) {
        Some(section) => coverage_lines(out, run, index, plan, section),
        None => entry(
            out,
            2,
            "coverage",
            "no record — this run predates the coverage and freshness evaluators, which \
             publish no measurement rather than inventing one after the fact",
            &[],
        ),
    }
    if let Some(section) = evaluation
        .get("freshness")
        .filter(|value| value.is_object())
    {
        freshness_lines(out, index, section);
    }
}

/// The grounding section: what the answer's citations were checked against,
/// or why nothing was checkable. The four verdicts stay distinct here as they
/// are in the record (`docs/contracts/run-output.md` §Plans).
fn grounding_lines(
    out: &mut String,
    run: &Path,
    index: usize,
    plan: &Value,
    evaluation: &Value,
    base: &str,
) {
    let p = |pointer: &str| cite_s(&format!("/plans/{index}{base}{pointer}"));
    let identity = format!(
        "{} v{} (configuration {})",
        text(&evaluation["evaluator"]["name"]),
        text(&evaluation["evaluator"]["version"]),
        short_hash(&text(&evaluation["evaluator"]["configuration_digest"])),
    );
    if let Some(reason) = evaluation["unevaluated"].as_str() {
        entry(
            out,
            2,
            "grounding",
            &format!("{identity} checked nothing: {reason}"),
            &[p("/unevaluated")],
        );
        return;
    }
    let citations = evaluation["citations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if citations.is_empty() {
        entry(
            out,
            2,
            "grounding",
            &format!(
                "{identity} found no CITATION lines in the answer: the suite required a \
                 cited answer and this one declared no citations, so nothing is scored"
            ),
            &[p("/citations")],
        );
        return;
    }
    // The tally is counted from the citation records themselves, not read
    // from `verdict_counts`: the record carries both, they must agree, and a
    // dossier that printed the claimed counts beside the citations without
    // comparing them would publish a self-contradicting document without
    // complaint. The recorded counts stay as a corroborating claim.
    let tally = |verdict: &str| {
        citations
            .iter()
            .filter(|citation| citation["verdict"] == verdict)
            .count()
    };
    let (supported, contradicted, uncovered, unavailable) = (
        tally("supported"),
        tally("contradicted"),
        tally("uncovered"),
        tally("unavailable"),
    );
    let counts = &evaluation["verdict_counts"];
    let counts_agree = [
        ("supported", supported),
        ("contradicted", contradicted),
        ("uncovered", uncovered),
        ("unavailable", unavailable),
    ]
    .iter()
    .all(|(key, counted)| counts[*key].as_u64() == Some(*counted as u64));
    let corroboration = if counts_agree {
        "verdict_counts records the same tally".to_owned()
    } else {
        format!(
            "COUNT MISMATCH: verdict_counts claims {} supported, {} contradicted, {} \
             uncovered, {} unavailable, which is not the tally of the citation records below",
            text(&counts["supported"]),
            text(&counts["contradicted"]),
            text(&counts["uncovered"]),
            text(&counts["unavailable"]),
        )
    };
    let mut cites = vec![p("/citations")];
    if !counts.is_null() {
        cites.push(p("/verdict_counts"));
    }
    entry(
        out,
        2,
        "grounding",
        &format!(
            "{identity} judged {} citation(s) against the exact text that entered the \
             window: {supported} supported, {contradicted} contradicted, {uncovered} \
             uncovered, {unavailable} unavailable, counted from the citation records below; \
             {corroboration}",
            citations.len(),
        ),
        &cites,
    );
    for (citation_index, citation) in citations.iter().enumerate() {
        let label = format!("#{} {}", citation_index + 1, text(&citation["verdict"]));
        let body = match citation["verdict"].as_str() {
            Some("supported") => format!(
                "{} — \"{}\" occurs at bytes {}-{} of the retained text (content {}); {}",
                text(&citation["url"]),
                text(&citation["quote"]),
                text(&citation["matched"]["start"]),
                text(&citation["matched"]["end"]),
                short_hash(&text(&citation["matched"]["content_hash"])),
                span_provenance(run, plan, &citation["matched"], &text(&citation["quote"]),),
            ),
            Some("unavailable") => {
                format!(
                    "{} — {}",
                    text(&citation["line"]),
                    text(&citation["reason"])
                )
            }
            _ => format!(
                "{} — \"{}\": {}",
                text(&citation["url"]),
                text(&citation["quote"]),
                text(&citation["reason"]),
            ),
        };
        entry(
            out,
            4,
            &label,
            &body,
            &[p(&format!("/citations/{citation_index}"))],
        );
    }
}

/// The coverage section: which predeclared rubric items the admitted window
/// covered, always named with the rubric it was measured against — "coverage
/// against rubric X", never a bare figure — or the stated reason nothing was
/// measured. Covered items print their byte spans and get the same
/// recheck-against-sealed-bytes treatment as supported citations.
fn coverage_lines(out: &mut String, run: &Path, index: usize, plan: &Value, section: &Value) {
    let p = |pointer: &str| cite_s(&format!("/plans/{index}/evaluation/coverage{pointer}"));
    let identity = format!(
        "{} v{} (configuration {})",
        text(&section["evaluator"]["name"]),
        text(&section["evaluator"]["version"]),
        short_hash(&text(&section["evaluator"]["configuration_digest"])),
    );
    let rubric = section["rubric"]
        .as_object()
        .map(|rubric| {
            format!(
                "rubric {}/{}",
                text(rubric.get("name").unwrap_or(&Value::Null)),
                text(rubric.get("version").unwrap_or(&Value::Null))
            )
        })
        .unwrap_or_else(|| "no declared rubric".to_owned());
    if let Some(reason) = section["unmeasured"].as_str() {
        entry(
            out,
            2,
            "coverage",
            &format!("{identity} measured nothing ({rubric}): {reason}"),
            &[p("/unmeasured")],
        );
        return;
    }
    entry(
        out,
        2,
        "coverage",
        &format!(
            "{identity} measured the admitted window against {rubric}: {} of {} declared \
             items covered, fraction {} — agreement with the rubric's author, not goodness",
            text(&section["covered_count"]),
            text(&section["item_count"]),
            text(&section["fraction"]),
        ),
        &[p("/covered_count"), p("/fraction"), p("/rubric")],
    );
    for (item_index, item) in section["items"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let covered = item["covered"] == Value::Bool(true);
        let label = format!(
            "#{} {}",
            item_index + 1,
            if covered { "covered" } else { "uncovered" }
        );
        let body = if covered {
            format!(
                "{} — \"{}\" occurs at bytes {}-{} of the retained text of {} (content {}); {}",
                text(&item["item"]),
                text(&item["matched"]["phrase"]),
                text(&item["matched"]["start"]),
                text(&item["matched"]["end"]),
                text(&item["matched"]["reference"]),
                short_hash(&text(&item["matched"]["content_hash"])),
                span_provenance(
                    run,
                    plan,
                    &item["matched"],
                    &text(&item["matched"]["phrase"]),
                ),
            )
        } else {
            format!("{} — {}", text(&item["item"]), text(&item["reason"]))
        };
        entry(out, 4, &label, &body, &[p(&format!("/items/{item_index}"))]);
    }
}

/// The freshness section: each admitted part's declared date against the
/// suite's as-of reference, every date with the provenance that declared it,
/// or the stated reason nothing was measured.
fn freshness_lines(out: &mut String, index: usize, section: &Value) {
    let p = |pointer: &str| cite_s(&format!("/plans/{index}/evaluation/freshness{pointer}"));
    let identity = format!(
        "{} v{} (configuration {})",
        text(&section["evaluator"]["name"]),
        text(&section["evaluator"]["version"]),
        short_hash(&text(&section["evaluator"]["configuration_digest"])),
    );
    if let Some(reason) = section["unmeasured"].as_str() {
        entry(
            out,
            2,
            "freshness",
            &format!("{identity} measured nothing: {reason}"),
            &[p("/unmeasured")],
        );
        // Fall through to the parts: no fraction exists, but the record's
        // own rule text promises every dated part's age and verdict still
        // stand — a dossier that stopped at the reason made that true only
        // of summary.json.
    } else {
        entry(
            out,
            2,
            "freshness",
            &format!(
                "{identity} measured {} part(s) against as_of {} with a maximum age of {} \
                 day(s): {} within the window, fraction {} — every date trusted as declared, \
                 not observed",
                text(&section["part_count"]),
                text(&section["as_of"]["date"]),
                text(&section["as_of"]["maximum_age_days"]),
                text(&section["fresh_count"]),
                text(&section["fraction"]),
            ),
            &[p("/fresh_count"), p("/fraction"), p("/as_of")],
        );
    }
    for (part_index, part) in section["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let label = format!(
            "#{} {}",
            part_index + 1,
            match part["within_maximum_age"].as_bool() {
                Some(true) => "within",
                Some(false) => "outside",
                None => "undated",
            }
        );
        let body = if part["declared_date"].is_null() {
            format!(
                "{} — no declared date; an undated part withholds the window's fraction",
                text(&part["reference"]),
            )
        } else {
            format!(
                "{} — declared {}, age {} day(s); declared by: {}",
                text(&part["reference"]),
                text(&part["declared_date"]),
                text(&part["age_days"]),
                text(&part["date_provenance"]),
            )
        };
        entry(out, 4, &label, &body, &[p(&format!("/parts/{part_index}"))]);
    }
}

/// Where a supported span's retained text can be obtained, and whether the
/// span was rechecked against it here. The retained text itself is never
/// published in `summary.json` — the record pins it by content hash only, and
/// the sealed responses that carry it may hold licensed content — so the
/// dossier names where the text lives instead of restating a span a reader
/// cannot resolve. Where the sealed bytes are present, the text is located in
/// them by its hash — received, never rebuilt — and the span is excised and
/// rechecked with the evaluator's own matcher; where they are not, the claim
/// is stated as resting on the record, exactly as the response-hash lines do.
fn span_provenance(run: &Path, plan: &Value, matched: &Value, quote: &str) -> String {
    let Some(hash) = matched["content_hash"].as_str() else {
        return "no content hash is recorded, so there is nothing to locate the retained \
                text by and the span cannot be rechecked"
            .to_owned();
    };
    let reference = plan["acquisition"]["response_ref"].as_str();
    let sealed = reference
        .filter(|reference| run.join(reference).exists())
        .map(|reference| (reference, run.join(reference)));
    let Some((reference, path)) = sealed else {
        return "the sealed response bytes are not present in this directory, so the span \
                is stated against the content hash and not rechecked here; the retained \
                text can be obtained by regenerating the run, which republishes responses/"
            .to_owned();
    };
    let located = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|response| string_hashing_to(&response, hash));
    let Some(retained) = located else {
        // The window text may lawfully differ from every sealed string: the
        // transform stage excises already-seen spans, and its invocation
        // record carries the hash of what survived. That record is the tie;
        // without it, the claim rests on the evaluation record alone.
        return if transform_output_hashes(plan).any(|output| output == hash) {
            format!(
                "the retained text is the transformed window text whose hash the plan's \
                 context line records; the sealed response ({reference}) holds the \
                 pre-transform bytes, so the span is stated against the content hash and \
                 not rechecked here"
            )
        } else {
            format!(
                "no text in the sealed response ({reference}) hashes to the record's \
                 content hash, so the span rests on the record's own claim and is not \
                 rechecked here"
            )
        };
    };
    let span = match (matched["start"].as_u64(), matched["end"].as_u64()) {
        (Some(start), Some(end)) => Some((start as usize, end as usize)),
        _ => None,
    };
    match span {
        Some((start, end))
            if commonmeasure_runtime::evaluate::span_excises_to_quote(
                &retained, start, end, quote,
            ) =>
        {
            format!(
                "the retained text is carried in the sealed response ({reference}), located \
                 by that hash, and the span excises to the quoted words, rechecked here"
            )
        }
        _ => format!(
            "SPAN MISMATCH: the retained text is carried in the sealed response \
             ({reference}), located by that hash, but the recorded span does not excise to \
             this quote, rechecked here"
        ),
    }
}

/// The first string anywhere in `value` whose SHA-256 is `hash`. The hash is
/// the receiver: whatever the response's shape, a string that hashes to the
/// record's pin *is* the retained text the record measured.
fn string_hashing_to(value: &Value, hash: &str) -> Option<String> {
    match value {
        Value::String(text) => (sha256_digest(text.as_bytes()) == hash).then(|| text.clone()),
        Value::Array(items) => items.iter().find_map(|item| string_hashing_to(item, hash)),
        Value::Object(map) => map.values().find_map(|item| string_hashing_to(item, hash)),
        _ => None,
    }
}

/// The output hashes the transform stage recorded for text it reshaped.
fn transform_output_hashes(plan: &Value) -> impl Iterator<Item = &str> {
    plan["processors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|invocation| invocation["stage"] == "transform")
        .flat_map(|invocation| {
            invocation["detail"]["removed_spans"]
                .as_array()
                .into_iter()
                .flatten()
        })
        .filter_map(|removal| removal["output_hash"].as_str())
}

fn verification_gloss(state: &str, capability: &str) -> &'static str {
    // The vocabulary is `docs/contracts/provider.md`; these are its one-line
    // readings, not new claims.
    match state {
        "planned" => "no acquisition evidence exists for this plan in this run",
        "fixture-tested" => "passes recorded or synthetic fixtures; not evidence of a call",
        "replay-tested" => {
            "the committed, redacted bytes of a dated live response were served \
                            through the real transport and sealed; no call reached the provider"
        }
        "spec-verified" => "documentation supports the implementation; no call evidence",
        // The state means the same thing either way — this run reached the
        // real supply and sealed exactly what came back — but nothing called
        // anybody for an invocation, and a reading that said "call" would
        // describe a crossing that did not happen.
        "live-verified" if capability == "invoke" => {
            "a dated execution of the real bundle completed and its exact \
                            result bytes are sealed beside this summary"
        }
        "live-verified" => {
            "a dated call against the real supply completed and its exact \
                            response bytes are sealed beside this summary"
        }
        "production-observed" => "sustained operator traffic confirms the behaviour",
        _ => "a state outside docs/contracts/provider.md",
    }
}

fn gap_reason_gloss(reason: &str) -> String {
    match reason {
        "evidence_missing" => "evidence missing".to_owned(),
        "inference_unavailable" => "inference unavailable".to_owned(),
        "coverage_gap" => "coverage shortfall".to_owned(),
        "policy_refused" => "policy refusal".to_owned(),
        other => other.to_owned(),
    }
}

/// What this plan executed, under whose declaration, and what came back.
///
/// The chain a reader needs is input bytes → invocation → result, and the
/// argument vector is where the first two meet: the job's own value is shown
/// at the position it entered the command line, distinguished from the
/// operator's declared text, because a produced result is only as
/// interpretable as the input that produced it.
fn invocation_lines(out: &mut String, index: usize, invocation: &Value) {
    let p = |suffix: &str| cite_s(&format!("/plans/{index}/acquisition/invocation{suffix}"));
    let declared = invocation["declared_name"]
        .as_str()
        .map(|name| format!("declares itself {name}"))
        .unwrap_or_else(|| "declares no name of its own".to_owned());
    entry(
        out,
        2,
        "skill",
        &format!(
            "{}, catalogued as {} in {}; {declared}. No version: the Agent Skill format has \
             no version key, so identity is that declared name plus the digests below — \
             SKILL.md {}, entrypoint {}, both taken from the bytes read immediately before \
             executing them",
            text(&invocation["root"]),
            text(&invocation["catalogue_name"]),
            text(&invocation["catalogue"]),
            short_hash(&text(&invocation["skill_md_sha256"])),
            short_hash(&text(&invocation["entrypoint_sha256"])),
        ),
        &[
            p("/declared_name"),
            p("/declared_version"),
            p("/skill_md_sha256"),
            p("/entrypoint_sha256"),
        ],
    );
    // The licence the bundle declares covers the procedure. What may be done
    // with what it printed is stated nowhere, which is why every source below
    // reads unknown.
    entry(
        out,
        2,
        "skill licence",
        &match invocation["declared_licence"].as_str() {
            Some(licence) => format!(
                "the bundle declares {licence} for itself. That licenses the procedure, not \
                 its output: nothing anywhere states what may be done with what it produced, \
                 so this run's source licence stays unknown"
            ),
            None => "the bundle declares no licence, and none is inferred from the fact that \
                     it is installed"
                .to_owned(),
        },
        &[p("/declared_licence")],
    );

    let argv = invocation["argv"].as_array().cloned().unwrap_or_default();
    let sources = invocation["argv_source"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let command: Vec<String> = argv
        .iter()
        .enumerate()
        .map(|(position, argument)| {
            let from = sources
                .get(position)
                .and_then(Value::as_str)
                .unwrap_or("unattributed");
            format!("{} [{from}]", text(argument))
        })
        .collect();
    entry(
        out,
        2,
        "command",
        &format!(
            "{} — each element tagged with where it came from: the operator's catalogue \
             declared every [operator] element, and the job supplied every [job] one. \
             Nothing was parsed by a shell, and the entrypoint resolved inside the bundle \
             root or nothing would have run",
            command.join("  ")
        ),
        &[p("/argv"), p("/argv_source")],
    );
    entry(
        out,
        2,
        "execution",
        &format!(
            "{} in {}, environment {}; {} after {} ms against a declared {} ms timeout and a \
             {}-byte output cap. {}",
            text(&invocation["interpreter"]),
            text(&invocation["cwd"]),
            text(&invocation["environment"]),
            match (
                invocation["exit_code"].as_i64(),
                text(&invocation["termination"]).as_str()
            ) {
                (Some(code), _) => format!("exited {code}"),
                (None, termination) =>
                    format!("ended by signal ({termination}), with no exit code"),
            },
            text(&invocation["duration_ms"]),
            text(&invocation["timeout_ms"]),
            text(&invocation["maximum_output_bytes"]),
            text(&invocation["limit_note"]),
        ),
        &[
            p("/interpreter"),
            p("/exit_code"),
            p("/duration_ms"),
            p("/limit_applied"),
        ],
    );
    entry(
        out,
        2,
        "streams",
        &format!(
            "standard output {} bytes hashing to {} — that stream alone is the result; \
             standard error {} bytes hashing to {}, sealed and hashed but never admitted as \
             content, because diagnostics must not enter a window wearing an answer's clothes",
            text(&invocation["stdout_bytes"]),
            short_hash(&text(&invocation["stdout_sha256"])),
            text(&invocation["stderr_bytes"]),
            short_hash(&text(&invocation["stderr_sha256"])),
        ),
        &[p("/stdout_sha256"), p("/stderr_sha256")],
    );
}

/// Which bytes this plan's record rests on, and whether they were recorded
/// replay or live capture. The per-acquisition `replay` record is the ground
/// truth: its presence means the sealed response was served from a recording,
/// and its absence on an executed acquisition means the bytes were obtained
/// at run time.
fn bytes_lines(
    out: &mut String,
    run: &Path,
    index: usize,
    plan: &Value,
    replay: Option<&Value>,
    plan_id: &str,
) {
    let p = |suffix: &str| cite_s(&format!("/plans/{index}{suffix}"));
    let Some(acquisition) = plan["acquisition"].as_object() else {
        entry(
            out,
            2,
            "bytes",
            "none — no acquisition was attempted, so no response exists to seal; the gaps \
             below say why",
            &[p("/acquisition")],
        );
        return;
    };
    let acquisition = Value::Object(acquisition.clone());

    // A quoted plan's evidence starts before its bytes: the price, billing
    // note and sealed quote legs preceded the purchase decision, and the
    // dossier shows them in that order whichever way the decision went.
    if let Some(quote) = acquisition["quote"].as_object() {
        let quote = Value::Object(quote.clone());
        let reported = |field: &str| {
            quote[field]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("no {field} reported"))
        };
        entry(
            out,
            2,
            "quote",
            &format!(
                "priced before purchase — preview {} (workflow {}); billing status {}; \
                 billing note: {}; balance before: {}; quoted charge {}",
                text(&quote["preview_id"]),
                text(&quote["workflow"]),
                reported("billing_status"),
                reported("billing_note"),
                reported("balance_before"),
                charge_text(&quote["charge"]),
            ),
            &[p("/acquisition/quote")],
        );
        entry(
            out,
            2,
            "quote sealed",
            &format!(
                "inspect leg to {} hashing to {}; preview leg to {} hashing to {}",
                text(&quote["inspect_ref"]),
                text(&quote["inspect_hash"]),
                text(&quote["preview_ref"]),
                text(&quote["preview_hash"]),
            ),
            &[
                p("/acquisition/quote/inspect_ref"),
                p("/acquisition/quote/preview_ref"),
            ],
        );
        entry(
            out,
            2,
            "purchase decision",
            &format!(
                "{} — {}",
                text(&quote["decision"]),
                text(&quote["decision_reason"])
            ),
            &[p("/acquisition/quote/decision")],
        );
        allowance_line(out, &quote["allowance"], p("/acquisition/quote/allowance"));
    }
    // An acquisition that is not quote-then-buy carries the gate's record at
    // the acquisition itself: the reservation of a declared published price,
    // the receipt's commit, or the explicit absence.
    if acquisition["quote"].is_null() {
        allowance_line(out, &acquisition["allowance"], p("/acquisition/allowance"));
    }
    // An invoked plan's evidence is what ran, before what came back: the
    // bundle's declared identity, the digests of the bytes executed, and the
    // command line those bytes were given — with the job's own value shown
    // where it entered it, which is the whole chain from input to result.
    if let Some(invocation) = acquisition["invocation"].as_object() {
        invocation_lines(out, index, &Value::Object(invocation.clone()));
    }
    // A quote nothing settled seals no receipt: the quote legs above are the
    // whole exchange, and the decision and gaps say why it stopped there.
    if acquisition["response_ref"].is_null() {
        entry(
            out,
            2,
            "bytes",
            "no settlement — the purchase was not made, so no receipt exists to seal; the \
             purchase decision above and the plan's gaps say why",
            &[p("/acquisition")],
        );
        return;
    }

    match acquisition["replay"].as_object() {
        Some(record) => {
            let record = Value::Object(record.clone());
            let mut cites = vec![p("/acquisition/replay")];
            if let Some(bindings) = replay.and_then(|replay| replay["bindings"].as_array())
                && let Some(position) = bindings
                    .iter()
                    .position(|binding| binding["plan"] == plan_id)
            {
                cites.push(format!("r:/bindings/{position}"));
            }
            // The binding is checked against the bytes where they exist: the
            // sealed response file is hashed here. Where `responses/` is not
            // published, the recorded-hash claim rests on the runtime's own
            // boolean, and the dossier says so instead of presenting the
            // producer's claim as verified.
            let recomputed = acquisition["response_ref"]
                .as_str()
                .and_then(|reference| response_hash(run, reference));
            let claimed_match = record["matches_recorded_input"] == Value::Bool(true);
            let verdict = match (recomputed, record["recorded_response_sha256"].as_str()) {
                (Some(actual), Some(recorded)) if actual == recorded => {
                    if claimed_match {
                        "the sealed response hash equals the recorded hash, recomputed here \
                         from the sealed bytes, so these are those bytes"
                            .to_owned()
                    } else {
                        format!(
                            "RECORD MISMATCH: the sealed bytes do hash to the recording \
                             ({recorded}), recomputed here, yet the runtime's record says \
                             they do not — the record and the bytes disagree"
                        )
                    }
                }
                (Some(actual), Some(recorded)) => format!(
                    "MISMATCH: the sealed response does not hash to the recording — the \
                     bytes hash to {actual}, recomputed here, not the {recorded} recorded — \
                     so this plan's bytes are not what the manifest names"
                ),
                (None, Some(recorded)) if claimed_match => format!(
                    "the sealed response bytes are not present in this directory, so the \
                     recorded hash ({recorded}) rests on the runtime's own \
                     matches_recorded_input record and is not verified here"
                ),
                (None, Some(recorded)) => format!(
                    "MISMATCH: the sealed response does not hash to the recording \
                     ({recorded} recorded) by the runtime's own record, and the bytes are \
                     not present here to recheck"
                ),
                (_, None) => "no recorded hash is present in the replay record to check the \
                     sealed response against"
                    .to_owned(),
            };
            entry(
                out,
                2,
                "bytes",
                &format!(
                    "recorded replay, not live capture — the response is the {} recording of \
                     {}, served from {} over loopback; {verdict}",
                    text(&record["captured_at"]),
                    text(&record["recorded_endpoint"]),
                    text(&record["source"]),
                ),
                &cites,
            );
            entry(
                out,
                2,
                "redactions",
                &text(&record["redactions"]),
                &[p("/acquisition/replay/redactions")],
            );
            // What the recording may be used for is the other half of its
            // provenance, and a reader deciding whether to trust or republish
            // these bytes needs it beside them rather than in a file they
            // have to know to open.
            entry(
                out,
                2,
                "permitted use",
                &text(&record["permitted_use"]),
                &[p("/acquisition/replay/permitted_use")],
            );
        }
        // A produced result is not a capture of anything. Nothing published
        // these bytes, so there is no origin to name and no date to carry;
        // what stands behind them is the invocation recorded above.
        None if acquisition["invocation"].is_object() => {
            entry(
                out,
                2,
                "bytes",
                &format!(
                    "produced result — the invocation's standard output, sealed to {} \
                     hashing to {}. Nothing published or dated these bytes: they did not \
                     exist until this run ran the bundle at {}",
                    text(&acquisition["response_ref"]),
                    text(&acquisition["response_hash"]),
                    text(&acquisition["endpoint"]),
                ),
                &[p("/acquisition/response_ref"), p("/acquisition/invocation")],
            );
        }
        None if text(&acquisition["endpoint"]).starts_with("file:") => {
            entry(
                out,
                2,
                "bytes",
                &format!(
                    "local capture — a run-time read of the operator's own supply at {}; \
                     no network call and no recording involved",
                    text(&acquisition["endpoint"])
                ),
                &[p("/acquisition/endpoint")],
            );
        }
        None => {
            entry(
                out,
                2,
                "bytes",
                &format!(
                    "live capture — fetched from {} when the run executed; no replay record \
                     is attached, so these bytes came from the provider, not a recording",
                    text(&acquisition["endpoint"])
                ),
                &[p("/acquisition/endpoint")],
            );
        }
    }

    let status = match acquisition["http_status"].as_u64() {
        Some(status) => format!("HTTP {status}"),
        None => "no HTTP status — this was not an HTTP exchange".to_owned(),
    };
    let request_id = match acquisition["provider_request_id"].as_str() {
        Some(id) => format!("provider request id {id}"),
        None => "no provider request id was reported".to_owned(),
    };
    entry(
        out,
        2,
        "sealed",
        &format!(
            "{status}; {} bytes sealed to {} hashing to {}; {} result(s); {request_id}",
            text(&acquisition["response_bytes"]),
            text(&acquisition["response_ref"]),
            text(&acquisition["response_hash"]),
            text(&acquisition["result_count"]),
        ),
        &[p("/acquisition")],
    );
    entry(
        out,
        2,
        "charge",
        &charge_text(&acquisition["charge"]),
        &[p("/acquisition/charge")],
    );
}

/// The principal's cumulative allowance at one acquisition — the second gate
/// beside the job's own cap, and the dossier keeps them visibly apart: the
/// cost ruling bounds one purchase, this line bounds the period. Prints
/// nothing when the record is absent (a corpus query, a skill invocation, or
/// a record predating contextops-run/v7).
fn allowance_line(out: &mut String, allowance: &Value, cite: String) {
    if allowance.is_null() {
        return;
    }
    let line = if allowance["consulted"].as_bool() != Some(true) {
        format!("not consulted — {}", text(&allowance["reason"]))
    } else if allowance["declared"].as_bool() != Some(true) {
        format!(
            "consulted for principal {} — {}",
            text(&allowance["principal"]),
            text(&allowance["reason"]),
        )
    } else {
        let mut line = format!(
            "principal {} — {}",
            text(&allowance["principal"]),
            text(&allowance["decision"]),
        );
        if let Some(reason) = allowance["reason"].as_str() {
            line.push_str(&format!(" — {reason}"));
        }
        for check in allowance["checks"].as_array().into_iter().flatten() {
            line.push_str(&format!(
                "; {} {}: {} spent of {} declared",
                text(&check["period"]),
                text(&check["period_key"]),
                money_text(&check["spent_before"]),
                money_text(&check["declared"]),
            ));
        }
        if let Some(settlement) = allowance["settlement"].as_object() {
            let outcome = settlement
                .get("note")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    format!(
                        "not recorded — {}",
                        settlement
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("no note")
                    )
                });
            line.push_str(&format!("; settlement: {outcome}"));
        }
        line
    };
    entry(out, 2, "allowance", &line, &[cite]);
}

/// A serialised [`commonmeasure_types::Money`] as "0.007000 USD", exact micros kept.
fn money_text(money: &Value) -> String {
    match money["micros"].as_u64() {
        Some(micros) => format!(
            "{}.{:06} {}",
            micros / 1_000_000,
            micros % 1_000_000,
            text(&money["currency"])
        ),
        None => "?".to_owned(),
    }
}

fn charge_text(charge: &Value) -> String {
    let native = charge["native"]
        .as_object()
        .map(|native| Value::Object(native.clone()));
    let basis = native.as_ref().map(|native| {
        // The note is the charge's own qualifier — why no comparable
        // money exists, or what a zero actually means (a trial-covered
        // purchase is a meter moving, not a price) — and dropping it
        // would leave the bare figure implying more than was observed.
        let note = native["note"]
            .as_str()
            .map(|note| format!(": {note}"))
            .unwrap_or_default();
        match native["basis"].as_str() {
            Some("observed") => {
                format!("observed from the provider's own response{note}")
            }
            Some("quoted") => format!("quoted, not observed{note}"),
            _ => "basis unknown".to_owned(),
        }
    });

    let mut parts = Vec::new();
    if let Some(money) = charge["money"].as_object() {
        let money = Value::Object(money.clone());
        parts.push(format!(
            "{} {}",
            money["micros"]
                .as_f64()
                .map(|micros| format!("{}", micros / 1_000_000.0))
                .unwrap_or_else(|| "?".to_owned()),
            text(&money["currency"]),
        ));
        // The native half usually restates a currency charge in the
        // provider's own unit, and repeating the same number would read as
        // two charges. It is folded away only when it *is* the same charge —
        // same unit and same amount — so a record whose halves disagree
        // displays the disagreement instead of hiding one side.
        let restates_money = |native: &Value| {
            native["unit"] == money["currency"]
                && match (native["amount"].as_f64(), money["micros"].as_f64()) {
                    (Some(amount), Some(micros)) => (amount * 1_000_000.0 - micros).abs() < 0.5,
                    _ => false,
                }
        };
        if let Some(native) = &native
            && !restates_money(native)
        {
            parts.push(format!(
                "{} {}",
                text(&native["amount"]),
                text(&native["unit"])
            ));
        }
    } else if let Some(native) = &native {
        parts.push(format!(
            "{} {}",
            text(&native["amount"]),
            text(&native["unit"])
        ));
    }

    match (parts.is_empty(), basis) {
        (true, _) => "not disclosed by the provider — an unknown price, never zero".to_owned(),
        (false, Some(basis)) => format!("{} ({basis})", parts.join("; ")),
        (false, None) => parts.join("; "),
    }
}

fn policy_lines(out: &mut String, index: usize, plan: &Value) {
    for (decision_index, decision) in plan["policy_decisions"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let body = match text(&decision["decision"]).as_str() {
            "admit"
                if !decision["gaps"]
                    .as_array()
                    .map(Vec::is_empty)
                    .unwrap_or(true) =>
            {
                format!(
                    "admitted, with a caveat on record: {}",
                    text(&decision["reason"])
                )
            }
            "admit" => format!("admitted: {}", text(&decision["reason"])),
            "refuse" => format!(
                "refused{}: {}",
                decision["source_url"]
                    .as_str()
                    .map(|url| format!(" {url}"))
                    .unwrap_or_default(),
                text(&decision["reason"])
            ),
            other => format!("{other}: {}", text(&decision["reason"])),
        };
        entry(
            out,
            2,
            "policy",
            &body,
            &[cite_s(&format!(
                "/plans/{index}/policy_decisions/{decision_index}"
            ))],
        );
    }
}

fn source_lines(out: &mut String, index: usize, plan: &Value) {
    let sources = plan["sources"].as_array().cloned().unwrap_or_default();
    if sources.is_empty() {
        return;
    }
    let admitted = sources
        .iter()
        .filter(|source| source["admitted"] == Value::Bool(true))
        .count();
    let all_unknown = sources
        .iter()
        .all(|source| source["licence"]["state"] == "unknown");
    let licences = if all_unknown {
        "; every licence unknown — no supplier declared one, and accessibility is not permission"
    } else {
        ""
    };
    entry(
        out,
        2,
        "sources",
        &format!(
            "{} discovered, {admitted} admitted{licences}",
            sources.len()
        ),
        &[cite_s(&format!("/plans/{index}/sources"))],
    );
    for (source_index, source) in sources.iter().enumerate() {
        let label = format!(
            "#{} {}",
            source_index + 1,
            if source["admitted"] == Value::Bool(true) {
                "admitted"
            } else {
                "refused"
            }
        );
        let mut body = format!(
            "{} — {}; {} tokens",
            text(&source["title"]),
            text(&source["url"]),
            text(&source["tokens"]),
        );
        match source["content_hash"].as_str() {
            Some(hash) => {
                let _ = write!(body, "; content {}", short_hash(hash));
            }
            None => {
                let _ = write!(body, "; no content hash — no text was retrieved to hash");
            }
        }
        if source["licence"]["state"] != "unknown" {
            let _ = write!(
                body,
                "; licence {}{}",
                text(&source["licence"]["state"]),
                source["licence"]["reference"]
                    .as_str()
                    .map(|reference| format!(" ({reference})"))
                    .unwrap_or_default()
            );
        }
        if source["admitted"] != Value::Bool(true) {
            let _ = write!(body, ". Why: {}", text(&source["admission_reason"]));
        }
        entry(
            out,
            4,
            &label,
            &body,
            &[cite_s(&format!("/plans/{index}/sources/{source_index}"))],
        );
    }
}

/// The processor invocations, grouped by stage: screening (admit) judged each
/// source before it entered, transform reshaped what was admitted. Every
/// count and span here is read from the invocation records the run sealed.
fn processor_lines(out: &mut String, index: usize, plan: &Value, evidence: Option<&EvidenceLog>) {
    let invocations = plan["processors"].as_array().cloned().unwrap_or_default();
    let admit = invocations
        .iter()
        .enumerate()
        .filter(|(_, invocation)| invocation["stage"] == "admit");
    // One screening line per processor identity, not one line crediting the
    // first processor with every admit-stage invocation: the day a second
    // admit processor is installed, each must be counted under its own name.
    let mut groups: Vec<Vec<(usize, &Value)>> = Vec::new();
    for (invocation_index, invocation) in admit {
        match groups
            .iter_mut()
            .find(|group| group[0].1["processor"] == invocation["processor"])
        {
            Some(group) => group.push((invocation_index, invocation)),
            None => groups.push(vec![(invocation_index, invocation)]),
        }
    }
    for group in &groups {
        let first = group[0].1;
        let refusals = group
            .iter()
            .filter(|(_, invocation)| invocation["decision"] != "admit")
            .count();
        let findings: usize = group
            .iter()
            .filter_map(|(_, invocation)| invocation["detail"]["findings"].as_array())
            .map(Vec::len)
            .sum();
        let mut cites = Vec::new();
        for (invocation_index, invocation) in group {
            cites.push(cite_s(&format!(
                "/plans/{index}/processors/{invocation_index}"
            )));
            cites.extend(cite_e(
                evidence.and_then(|log| log.processor_seq(invocation)),
            ));
        }
        entry(
            out,
            2,
            "screening",
            &format!(
                "{} v{} (configuration {}) judged {} source text(s) before admission: {}, {}. \
                 Method: {}. Declared blind spots: {}",
                text(&first["processor"]["name"]),
                text(&first["processor"]["version"]),
                short_hash(&text(&first["processor"]["configuration_digest"])),
                group.len(),
                if refusals == 0 {
                    "all admitted".to_owned()
                } else {
                    format!("{refusals} refused")
                },
                if findings == 0 {
                    "no findings".to_owned()
                } else {
                    format!("{findings} finding(s)")
                },
                text(&first["method"]),
                first["blind_spots"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(text)
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            &cites,
        );
    }

    for (invocation_index, invocation) in invocations.iter().enumerate() {
        if invocation["stage"] != "transform" {
            continue;
        }
        let detail = &invocation["detail"];
        let mut cites = vec![cite_s(&format!(
            "/plans/{index}/processors/{invocation_index}"
        ))];
        cites.extend(cite_e(
            evidence.and_then(|log| log.processor_seq(invocation)),
        ));
        let mut body = format!(
            "{} v{}: {} of {} retrieved tokens entered the window; ordering: {}",
            text(&invocation["processor"]["name"]),
            text(&invocation["processor"]["version"]),
            text(&detail["tokens_out"]),
            text(&detail["tokens_in"]),
            text(&detail["ordering"]["basis"]),
        );
        for removed in detail["removed_sources"].as_array().into_iter().flatten() {
            let _ = write!(body, ". Removed whole source {}", text(removed));
        }
        for removal in detail["removed_spans"].as_array().into_iter().flatten() {
            let spans: Vec<String> = removal["spans"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|span| {
                    format!(
                        "chars {}-{} ({} words, first kept in {} at chars {}-{})",
                        text(&span["start"]),
                        text(&span["end"]),
                        text(&span["words"]),
                        text(&span["first_kept"]["reference"]),
                        text(&span["first_kept"]["start"]),
                        text(&span["first_kept"]["end"]),
                    )
                })
                .collect();
            let _ = write!(
                body,
                ". From {} it removed already-seen spans: {}; the surviving text hashes to {}",
                text(&removal["reference"]),
                spans.join("; "),
                short_hash(&text(&removal["output_hash"])),
            );
        }
        entry(out, 2, "context", &body, &cites);
    }
}

/// The verify-stage invocations: the fidelity verifier's per-claim verdicts,
/// each claim rechecked against the answer the plan carries, and the judge's
/// agreement with them. Printed after the grounding section because the
/// verifier's citation-backed claims refer to its verdicts.
/// The attest stage: where the plan's labelled output is, what the label
/// claims and how it validated, so a reader finds the label without opening
/// the summary. An abstention names the gap.
fn provenance_lines(out: &mut String, index: usize, plan: &Value, evidence: Option<&EvidenceLog>) {
    let invocations = plan["processors"].as_array().cloned().unwrap_or_default();
    for (invocation_index, invocation) in invocations.iter().enumerate() {
        if invocation["stage"] != "attest" {
            continue;
        }
        let base = format!("/plans/{index}/processors/{invocation_index}");
        let p = |suffix: &str| cite_s(&format!("{base}{suffix}"));
        let mut cites = vec![p("")];
        cites.extend(cite_e(
            evidence.and_then(|log| log.processor_seq(invocation)),
        ));
        let identity = format!(
            "{} v{} (configuration {})",
            text(&invocation["processor"]["name"]),
            text(&invocation["processor"]["version"]),
            short_hash(&text(&invocation["processor"]["configuration_digest"])),
        );
        let detail = &invocation["detail"];
        let ingredients = detail["manifest_definition"]["ingredients"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        match invocation["outputs"]
            .as_array()
            .and_then(|outputs| outputs.first())
        {
            Some(label) => {
                cites.push(p("/outputs/0"));
                cites.push(p("/detail/validation"));
                entry(
                    out,
                    2,
                    "provenance",
                    &format!(
                        "{identity} labelled the answer: {} (sha256 {}) carries a C2PA \
                         manifest with {ingredients} ingredient(s) by content hash and grade, \
                         validated on read-back as {}. Read it back with `commonmeasure \
                         provenance <run>/{} --run <run>`",
                        text(&label["reference"]),
                        short_hash(&text(&label["content_hash"])),
                        text(&detail["validation"]["summary"]),
                        text(&label["reference"]),
                    ),
                    &cites,
                );
            }
            None => {
                cites.push(p("/gaps"));
                entry(
                    out,
                    2,
                    "provenance",
                    &format!(
                        "{identity} built the manifest ({ingredients} ingredient(s)) and did not \
                         publish a label: {}",
                        invocation["gaps"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|gap| text(&gap["detail"]))
                            .collect::<Vec<_>>()
                            .join(" ")
                    ),
                    &cites,
                );
            }
        }
    }
}

fn fidelity_lines(out: &mut String, index: usize, plan: &Value, evidence: Option<&EvidenceLog>) {
    let invocations = plan["processors"].as_array().cloned().unwrap_or_default();
    for (invocation_index, invocation) in invocations.iter().enumerate() {
        if invocation["stage"] != "verify" {
            continue;
        }
        let base = format!("/plans/{index}/processors/{invocation_index}");
        let p = |suffix: &str| cite_s(&format!("{base}{suffix}"));
        let mut cites = vec![p("")];
        cites.extend(cite_e(
            evidence.and_then(|log| log.processor_seq(invocation)),
        ));
        let identity = format!(
            "{} v{} (configuration {})",
            text(&invocation["processor"]["name"]),
            text(&invocation["processor"]["version"]),
            short_hash(&text(&invocation["processor"]["configuration_digest"])),
        );
        let detail = &invocation["detail"];
        match invocation["processor"]["name"].as_str() {
            Some("fidelity-verifier") => {
                let claims = detail["claims"].as_array().cloned().unwrap_or_default();
                let counts = &detail["claim_counts"];
                cites.push(p("/detail/claims"));
                cites.push(p("/detail/claim_counts"));
                entry(
                    out,
                    2,
                    "fidelity",
                    &format!(
                        "{identity} segmented the answer into {} claim(s): {} supported, {} \
                         contradicted, {} unsupported; supported fraction {} — a verbatim \
                         span of the window backs the claim, which is not a finding that it \
                         is true, and a paraphrase of the window is invisible to it. \
                         Method: {}",
                        claims.len(),
                        text(&counts["supported"]),
                        text(&counts["contradicted"]),
                        text(&counts["unsupported"]),
                        text(&detail["supported_fraction"]),
                        text(&invocation["method"]),
                    ),
                    &cites,
                );
                let answer = plan["answer"].as_str().unwrap_or_default();
                for (claim_index, claim) in claims.iter().enumerate() {
                    let (start, end) = (
                        claim["start"].as_u64().unwrap_or_default() as usize,
                        claim["end"].as_u64().unwrap_or_default() as usize,
                    );
                    let recheck = match answer.get(start..end) {
                        Some(excised) if excised == text(&claim["text"]) => format!(
                            "the claim excises from the answer at bytes {start}-{end}, \
                             rechecked here"
                        ),
                        _ => format!(
                            "CLAIM MISMATCH: bytes {start}-{end} of the answer do not excise \
                             to this claim"
                        ),
                    };
                    let body = match claim["verdict"].as_str() {
                        Some("supported") => {
                            let evidence = &claim["evidence"];
                            format!(
                                "\"{}\" — {} word(s) of it occur at bytes {}-{} of the \
                                 retained text of {} (content {}), by {}; {}",
                                text(&claim["text"]),
                                text(&evidence["words"]),
                                text(&evidence["start"]),
                                text(&evidence["end"]),
                                text(&evidence["reference"]),
                                short_hash(&text(&evidence["content_hash"])),
                                match claim["basis"].as_str() {
                                    Some("citation") => format!(
                                        "citation #{}",
                                        claim["citation"].as_u64().unwrap_or_default() + 1
                                    ),
                                    _ => "a run of the claim's own words".to_owned(),
                                },
                                recheck,
                            )
                        }
                        _ => format!(
                            "\"{}\" — {}; {}",
                            text(&claim["text"]),
                            text(&claim["reason"]),
                            recheck
                        ),
                    };
                    entry(
                        out,
                        4,
                        &format!("#{} {}", claim_index + 1, text(&claim["verdict"])),
                        &body,
                        &[p(&format!("/detail/claims/{claim_index}"))],
                    );
                }
            }
            Some("fidelity-judge") => {
                if detail["judged"] != Value::Bool(true) {
                    cites.push(p("/detail/reason"));
                    cites.push(p("/gaps"));
                    entry(
                        out,
                        2,
                        "judge",
                        &format!("{identity} judged nothing: {}", text(&detail["reason"])),
                        &cites,
                    );
                    continue;
                }
                let agreement = &detail["agreement"];
                let judge = &detail["judge"];
                let matrix = &agreement["matrix"];
                let row = |verifier: &str| {
                    format!(
                        "of the claims the verifier found {verifier}, the judge said {} \
                         supported, {} contradicted, {} unsupported, {} unreadable",
                        text(&matrix[verifier]["supported"]),
                        text(&matrix[verifier]["contradicted"]),
                        text(&matrix[verifier]["unsupported"]),
                        text(&matrix[verifier]["unavailable"]),
                    )
                };
                cites.push(p("/detail/agreement"));
                cites.push(p("/detail/judge"));
                entry(
                    out,
                    2,
                    "judge",
                    &format!(
                        "{identity} asked {} through {} for a verdict per claim over the same \
                         window the answer model received: it agreed with the verifier on {} \
                         of {} claim(s) compared, agreement fraction {}; {}; {}; {}. The \
                         judge's verdict is the model's statement about the window, recorded \
                         as {}, and is never ground truth",
                        judge["executed_model"]
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| text(&judge["requested_model"])),
                        text(&judge["gateway"]),
                        text(&agreement["agreed"]),
                        text(&agreement["compared"]),
                        text(&agreement["fraction"]),
                        row("supported"),
                        row("contradicted"),
                        row("unsupported"),
                        text(&invocation["assurance"]),
                    ),
                    &cites,
                );
                for verdict in detail["verdicts"].as_array().into_iter().flatten() {
                    let claim = verdict["claim"].as_u64().unwrap_or_default();
                    entry(
                        out,
                        4,
                        &format!("#{} judge {}", claim + 1, text(&verdict["judge"])),
                        &format!(
                            "verifier {}; {}",
                            text(&verdict["verifier"]),
                            match verdict["reason"].as_str() {
                                Some(reason) if !reason.is_empty() =>
                                    format!("the judge's reason: \"{reason}\""),
                                _ => "the judge gave no reason".to_owned(),
                            }
                        ),
                        &[p(&format!("/detail/verdicts/{claim}"))],
                    );
                }
            }
            _ => {
                entry(
                    out,
                    2,
                    "verify",
                    &format!("{identity} ran; method: {}", text(&invocation["method"])),
                    &cites,
                );
            }
        }
    }
}

fn inference_lines(out: &mut String, index: usize, plan: &Value) {
    let p = |suffix: &str| cite_s(&format!("/plans/{index}{suffix}"));
    let Some(inference) = plan["inference"].as_object() else {
        entry(
            out,
            2,
            "inference",
            "not attempted — no inference record exists for this plan; the gaps below say why",
            &[p("/inference")],
        );
        return;
    };
    let inference = Value::Object(inference.clone());
    let requested = text(&inference["requested_model"]);
    let executed = text(&inference["executed_model"]);
    let model = if requested == executed {
        format!("model {executed}, executed as requested")
    } else {
        format!("model requested {requested} but executed {executed}")
    };
    entry(
        out,
        2,
        "inference",
        &format!(
            "{} via the {} gateway; {model}; route: {}; {} tokens in, {} out, {} ms; gateway \
             request id {}",
            text(&inference["status"]),
            text(&inference["gateway"]),
            text(&inference["route_reason"]),
            text(&inference["input_tokens"]),
            text(&inference["output_tokens"]),
            text(&inference["latency_ms"]),
            text(&inference["provider_request_id"]),
        ),
        &[p("/inference")],
    );
    for field in inference["unavailable_fields"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let field = text(field);
        let explanation = match field.as_str() {
            "executed_provider" => "the gateway's response did not name the upstream provider \
                                    that actually served this call"
                .to_owned(),
            other => format!("the gateway did not report {other}"),
        };
        entry(
            out,
            2,
            "unavailable",
            &format!("{field} — {explanation}"),
            &[p("/inference/unavailable_fields")],
        );
    }
    // Reported, but not in a shape this runtime reads. Said apart from
    // "not reported" so a reader chasing a missing figure goes to the
    // gateway that sent something rather than to one that sent nothing.
    for field in inference["unreadable_fields"]
        .as_array()
        .into_iter()
        .flatten()
    {
        entry(
            out,
            2,
            "unreadable",
            &format!(
                "{} — the gateway reported it in a shape this runtime does not read, so the \
                 value stays unknown",
                text(field)
            ),
            &[p("/inference/unreadable_fields")],
        );
    }
}

fn selection_block(out: &mut String, summary: &Value) {
    let selection = &summary["selection"];
    let body = match selection["plan_id"].as_str() {
        Some(plan_id) => format!(
            "{plan_id}, chosen by {}. {}",
            text(&selection["method"]),
            text(&selection["explanation"])
        ),
        None => {
            let missing: Vec<String> = selection["unavailable_inputs"]
                .as_array()
                .into_iter()
                .flatten()
                .map(text)
                .collect();
            format!(
                "no plan was selected. {}{}",
                text(&selection["explanation"]),
                if missing.is_empty() {
                    String::new()
                } else {
                    format!(" Unmeasured objective terms: {}.", missing.join(", "))
                }
            )
        }
    };
    entry(out, 0, "selection", &body, &[cite_s("/selection")]);

    let plans = summary["plans"].as_array().cloned().unwrap_or_default();
    if !plans.is_empty() && plans.iter().all(|plan| plan["evaluation"].is_null()) {
        // Same guard as the per-plan line: a pre-evaluator summary carries no
        // `evaluation` key, and absence has no record to cite.
        let cites: Vec<String> = plans[0]
            .as_object()
            .is_some_and(|plan| plan.contains_key("evaluation"))
            .then(|| cite_s("/plans/0/evaluation"))
            .into_iter()
            .collect();
        entry(
            out,
            0,
            "evaluation",
            "none recorded — no plan carries an evaluation record, so this run predates \
             the grounding evaluator; it publishes no quality, coverage or citation \
             measurement rather than a fabricated one",
            &cites,
        );
    }
    if let Some(fairness) = summary["run"]["fairness"].as_str() {
        entry(out, 0, "fairness", fairness, &[cite_s("/run/fairness")]);
    }
}
