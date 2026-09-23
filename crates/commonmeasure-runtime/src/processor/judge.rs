//! The fidelity judge: an optional `verify`-stage processor that asks a model,
//! through the configured inference gateway, whether each claim of an answer
//! is supported by the window, and measures its agreement with the
//! deterministic verifier's verdicts.
//!
//! The judge's verdict is the model's statement about the window text, so
//! the record is `declared`, never `observed`, and it is never ground truth:
//! the verifier's verdicts stand beside it and the agreement figure is the
//! measurement. The judge sees exactly the window the answer model saw,
//! rendered by the same request builder, and runs only when the suite opts
//! in, because a suite that declares a judge is a different experiment from
//! one that does not.

use commonmeasure_types::canonical::sha256_digest;
use std::sync::LazyLock;

use chrono::Utc;
use commonmeasure_inference::{ContextPart, InferenceBackend, InferenceError, InferenceRequest};
use commonmeasure_types::{Decision, Gap, GapReason, ModelPlan};
use serde_json::{Value, json};
use uuid::Uuid;

use super::fidelity::{Claim, ClaimVerdict};
use super::{
    ArtefactRef, Determinism, FailBehaviour, FailureByMode, IN_PROCESS, INVOCATION_VERSION,
    Invocation, Permissions, ProcessorManifest, Stage,
};

pub const NAME: &str = "fidelity-judge";
pub const VERSION: &str = "1";

/// The system message the judge model receives. It defines the verdict
/// vocabulary and the exact line form the parser reads, so the two travel
/// together under one digest.
pub const INSTRUCTION: &str = "You are checking the claims of an answer against supplied \
     context. Judge each numbered claim from the SUPPLIED CONTEXT alone, using no outside \
     knowledge: supported means the context states what the claim says, in the same or \
     other words; contradicted means the context states something incompatible with the \
     claim; unsupported means the context neither states nor contradicts it. Reply with \
     one line per claim, in exactly this form and nothing else:\n\
     CLAIM <number>: <supported|contradicted|unsupported> :: \"<one sentence naming the \
     context that decided it>\"";

/// The rule set, stated canonically; the configuration digest covers it and
/// the instruction. A commit that changes what the judge is asked or how its
/// answer is read changes this text and `VERSION` together.
const RULES: &str = "fidelity-judge/1: one gateway exchange per plan under the suite's model \
     plan: the instruction as the system message, the claims numbered from 1 as the \
     prompt, and the window parts rendered exactly as the answer model received them. \
     Parse `CLAIM <n>: <verdict> :: \"<reason>\"` lines from the reply, the marker read \
     case-insensitively behind any list, numbering, blockquote or emphasis decoration; a \
     claim with no readable line, or a verdict outside supported, contradicted and \
     unsupported, is unavailable; the first line for a number wins. Agreement is over \
     the claims the judge gave a verdict: agreed = the judge's verdict equals the \
     verifier's, the fraction is agreed over compared and null where nothing was \
     compared; the matrix counts judge verdicts under each verifier verdict. The judge's \
     verdict is never ground truth and never replaces the verifier's.";

static MANIFEST: LazyLock<ProcessorManifest> = LazyLock::new(|| ProcessorManifest {
    name: NAME,
    version: VERSION,
    stage: Stage::Verify,
    capability: "claim-support-judgement",
    implementation: IN_PROCESS,
    configuration_digest: sha256_digest(format!("{RULES}{INSTRUCTION}").as_bytes()),
    permissions: Permissions {
        // The model is reached through the configured gateway, the same seam
        // inference uses; the processor holds no credential of its own.
        network: true,
        filesystem: false,
        model: true,
        credentials: false,
        raw_content: true,
    },
    determinism: Determinism::NonDeterministic,
    limits: "one gateway exchange per plan, bounded by the transport timeout; no retry",
    failure_by_mode: FailureByMode {
        // A judge that cannot run leaves the verifier's record standing, with
        // a gap naming the dependency.
        strict: FailBehaviour::FailOpen,
        prefer: FailBehaviour::FailOpen,
        observe: FailBehaviour::FailOpen,
    },
    evidence_format: INVOCATION_VERSION,
});

pub fn manifest() -> &'static ProcessorManifest {
    &MANIFEST
}

const BLIND_SPOTS: &[&str] = &[
    "The judge's verdict is a model's statement about the window text, recorded as declared; \
     it is measured against the deterministic verifier and is never ground truth.",
    "Disagreement on claims the verifier found unsupported is expected where the claim \
     paraphrases the window: the verifier cannot see paraphrase and the judge can claim to.",
    "The judge may be wrong in either direction, and nothing here detects it: the agreement \
     figure measures consistency between two checks, not the truth of either.",
    "The exchange's charge in currency is unknown: the inference client does not read monetary charges.",
];

/// Judge one plan's claims. Always returns an invocation: a judge that could
/// not run records why, so a reader can tell "not judged, and why" from
/// "nothing judged it".
pub fn invoke(
    backend: Option<&dyn InferenceBackend>,
    run_id: Uuid,
    model_plan: &ModelPlan,
    parts: &[ContextPart],
    claims: &[Claim],
) -> Invocation {
    let started_at = Utc::now();
    let mut inputs = vec![ArtefactRef {
        reference: "claims".to_owned(),
        content_hash: Some(sha256_digest(claims_prompt(claims).as_bytes())),
        tokens: None,
    }];
    inputs.extend(parts.iter().map(|part| ArtefactRef {
        reference: part.source_ref.clone(),
        content_hash: Some(part.content_hash.clone()),
        tokens: None,
    }));

    let abstain = |reason: GapReason, detail: String, inputs: Vec<ArtefactRef>| {
        Invocation::new(
            manifest(),
            started_at,
            Decision::Abstain,
            "not judged",
            inputs,
            Vec::new(),
            json!({"judged": false, "reason": detail}),
            vec![Gap::new(reason, detail.clone())],
            BLIND_SPOTS.to_vec(),
        )
        .declared()
    };

    if claims.is_empty() {
        return abstain(
            GapReason::EvidenceMissing,
            "The verifier segmented no claims from the answer, so there is nothing to judge."
                .to_owned(),
            inputs,
        );
    }
    if parts.is_empty() {
        return abstain(
            GapReason::EvidenceMissing,
            "The window is empty, so there is no supplied context to judge the claims against."
                .to_owned(),
            inputs,
        );
    }
    let Some(backend) = backend else {
        return abstain(
            GapReason::InferenceUnavailable,
            format!(
                "No inference gateway is configured, so the judge could not run. Set {} to an \
                 OpenAI-compatible chat-completions endpoint.",
                commonmeasure_inference::ENDPOINT_VARIABLE
            ),
            inputs,
        );
    };

    let request = InferenceRequest {
        run_id,
        model_plan: model_plan.clone(),
        system: INSTRUCTION.to_owned(),
        prompt: claims_prompt(claims),
        context: parts.to_vec(),
    };
    let response = match backend.infer(&request) {
        Ok(response) => response,
        Err(error) => {
            return abstain(
                match error {
                    InferenceError::Unavailable(_) => GapReason::InferenceUnavailable,
                    _ => GapReason::EvidenceMissing,
                },
                format!("The judge's gateway exchange failed: {error}"),
                inputs,
            );
        }
    };

    let judged = parse_verdicts(&response.output, claims.len());
    let mut verdicts = Vec::new();
    let mut compared = 0usize;
    let mut agreed = 0usize;
    let mut matrix = serde_json::Map::new();
    for verifier in [
        ClaimVerdict::Supported,
        ClaimVerdict::Contradicted,
        ClaimVerdict::Unsupported,
    ] {
        matrix.insert(
            verifier.as_str().to_owned(),
            json!({"supported": 0, "contradicted": 0, "unsupported": 0, "unavailable": 0}),
        );
    }
    for (index, claim) in claims.iter().enumerate() {
        let (judge, reason) = judged[index].clone();
        let judge_key = judge.map(ClaimVerdict::as_str).unwrap_or("unavailable");
        let cell = &mut matrix[claim.verdict.as_str()][judge_key];
        *cell = json!(cell.as_u64().unwrap_or_default() + 1);
        let agrees = judge.map(|judge| judge == claim.verdict);
        if let Some(agrees) = agrees {
            compared += 1;
            if agrees {
                agreed += 1;
            }
        }
        verdicts.push(json!({
            "claim": index,
            "verifier": claim.verdict,
            "judge": judge_key,
            "agrees": agrees,
            "reason": reason,
        }));
    }
    let fraction = if compared == 0 {
        Value::Null
    } else {
        json!(agreed as f64 / compared as f64)
    };

    Invocation::new(
        manifest(),
        started_at,
        Decision::Abstain,
        "one gateway exchange asking the suite's model for a verdict per claim over the same \
         window the answer model received, parsed deterministically and compared with the \
         verifier's verdicts",
        inputs,
        vec![ArtefactRef {
            reference: "judge-response".to_owned(),
            content_hash: Some(sha256_digest(response.output.as_bytes())),
            tokens: response.output_tokens,
        }],
        json!({
            "judged": true,
            "judge": {
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
            },
            // A processor that spends must carry a cost field; this one's
            // charge is unknown, not zero (docs/FAIL-POLICY.md §7).
            "cost": {
                "money": Value::Null,
                "note": "the inference client does not read monetary charges; the charge in currency is \
                         unknown",
            },
            "response": response.output,
            "verdicts": verdicts,
            "agreement": {
                "compared": compared,
                "agreed": agreed,
                "fraction": fraction,
                "matrix": matrix,
            },
        }),
        Vec::new(),
        BLIND_SPOTS.to_vec(),
    )
    .declared()
}

/// The prompt: the claims numbered from 1, exactly as verified.
fn claims_prompt(claims: &[Claim]) -> String {
    let mut prompt = String::from("CLAIMS");
    for (index, claim) in claims.iter().enumerate() {
        prompt.push_str(&format!("\nCLAIM {}: {}", index + 1, claim.text));
    }
    prompt
}

const MARKER: &str = "CLAIM";

/// One verdict slot per claim: the judge's verdict where a line was readable,
/// and the reason text the line carried (or why it was unreadable).
fn parse_verdicts(output: &str, count: usize) -> Vec<(Option<ClaimVerdict>, String)> {
    let mut slots: Vec<Option<(Option<ClaimVerdict>, String)>> = vec![None; count];
    for line in output.lines() {
        let Some((number, verdict, reason)) = verdict_line(line) else {
            continue;
        };
        if number == 0 || number > count {
            continue;
        }
        let slot = &mut slots[number - 1];
        if slot.is_none() {
            *slot = Some((verdict, reason));
        }
    }
    slots
        .into_iter()
        .map(|slot| {
            slot.unwrap_or_else(|| {
                (
                    None,
                    "no readable `CLAIM <n>: <verdict>` line names this claim".to_owned(),
                )
            })
        })
        .collect()
}

/// `(number, verdict, reason)` for a line of the form `CLAIM n: verdict ::
/// "reason"`, behind any list, numbering or emphasis decoration; `verdict`
/// is `None` for a word outside the vocabulary, with the reason saying so.
fn verdict_line(line: &str) -> Option<(usize, Option<ClaimVerdict>, String)> {
    let mut rest = line.trim();
    loop {
        let stripped = rest
            .trim_start_matches(['-', '*', '+', '>', '#', '`', '_', ' ', '\t'])
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .trim_start_matches(['.', ')', ' ', '\t']);
        if stripped == rest {
            break;
        }
        rest = stripped;
    }
    if !rest.get(..MARKER.len())?.eq_ignore_ascii_case(MARKER) {
        return None;
    }
    let rest = rest[MARKER.len()..].trim_start();
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let number: usize = digits.parse().ok()?;
    let rest = rest[digits.len()..]
        .trim_start_matches(['*', '`', '_'])
        .trim_start();
    let rest = rest
        .strip_prefix(':')?
        .trim_start_matches(['*', '`', '_', ' ']);
    let (word, reason) = match rest.split_once("::") {
        Some((word, reason)) => (word, reason.trim().trim_matches('"').to_owned()),
        None => (rest, String::new()),
    };
    let word = word.trim().trim_matches(['*', '`', '_', ' ', '.']);
    let verdict = match word.to_ascii_lowercase().as_str() {
        "supported" => Some(ClaimVerdict::Supported),
        "contradicted" => Some(ClaimVerdict::Contradicted),
        "unsupported" => Some(ClaimVerdict::Unsupported),
        other => {
            return Some((
                number,
                None,
                format!("the verdict word \"{other}\" is outside the vocabulary"),
            ));
        }
    };
    Some((number, verdict, reason))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_lines_are_read_behind_decoration_and_out_of_order() {
        let output = "Here are my verdicts:\n\n2. **CLAIM 2:** contradicted :: \"the source \
                      says twice a year\"\n- claim 1: Supported :: \"stated verbatim\"\nCLAIM 3: \
                      maybe :: \"unsure\"\nCLAIM 9: supported\nCLAIM 1: unsupported";
        let parsed = parse_verdicts(output, 4);
        assert_eq!(parsed[0].0, Some(ClaimVerdict::Supported));
        assert_eq!(parsed[0].1, "stated verbatim");
        assert_eq!(parsed[1].0, Some(ClaimVerdict::Contradicted));
        assert_eq!(parsed[2].0, None);
        assert!(parsed[2].1.contains("outside the vocabulary"));
        assert_eq!(parsed[3].0, None);
        assert!(parsed[3].1.contains("no readable"));
    }

    #[test]
    fn the_rule_text_and_the_digest_move_with_the_version() {
        assert!(RULES.starts_with(&format!("{NAME}/{VERSION}:")));
        assert_eq!(
            manifest().configuration_digest,
            "sha256:67d5ec72614310aee0f740fca4571c3decd25881c70477397b6357e75f547afc"
        );
    }
}
