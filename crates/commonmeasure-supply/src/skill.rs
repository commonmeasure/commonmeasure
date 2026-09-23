//! Skill supply: a declared bundle the operator admits, executed once.
//!
//! A skill is a named unit of procedure supplied by a third party. Its result
//! is *produced*, not retrieved: nothing published it, nobody priced it, and
//! no date attaches to it. That is why `invoke` is its own capability rather
//! than `search` under another name, and why this adapter implements nothing
//! else — a local execution has no supplier price to quote, no corpus to
//! query and no rights it could declare over what it printed.
//!
//! Two things the Agent Skill format does not provide shape everything here.
//! It declares **no version**: the real validator's frontmatter allow-list is
//! `name`, `description`, `license`, `allowed-tools`, `metadata`, so identity
//! is the declared name plus a digest of the bytes this runtime actually read
//! and executed. And it declares **no machine-readable invocation contract**:
//! scripts are described in prose for a model to run, so an adapter cannot
//! learn how to invoke a skill *from* the skill. The operator declares it, in
//! a catalogue, and every invocation input therefore carries operator
//! provenance and is never the skill's own claim.
//!
//! Nothing here is a sandbox and this module does not pretend otherwise. The
//! child runs as the operator, on the operator's filesystem. What is enforced
//! is what can honestly be enforced at this boundary: the entrypoint resolves
//! inside the declared root or nothing is executed, the interpreter is named
//! absolutely, the argument vector is explicit with no shell parsing it, the
//! environment is emptied so the child cannot read the operator's provider
//! credentials, the wall clock is bounded by a kill, and a result past the
//! declared cap is refused whole rather than truncated.

use commonmeasure_types::canonical::sha256_digest;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use commonmeasure_types::{AcquisitionCharge, ContextEnvelope, LicenceState, ProviderCapability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Acquisition, Containment, SupplyAdapter, SupplyError};

pub(crate) const CAPABILITIES: &[ProviderCapability] = &[ProviderCapability::Invoke];

/// Names the operator's skill catalogue for `supplier_from_environment`.
pub const CATALOGUE_VARIABLE: &str = "COMMONMEASURE_SKILL_CATALOGUE";

/// Every skill is its own provider, named `skill:<catalogue name>`, so
/// eligibility, allow- and deny-provider policy, one plan per provider and
/// the router all work on skills unchanged.
pub const SKILL_PROVIDER_PREFIX: &str = "skill:";

/// The provider name of the operator's own corpus: internal supply, not a
/// supplier, wherever a rule tells the two apart.
pub const INTERNAL_PROVIDER: &str = "internal";

/// The placeholder an operator writes where the job's own value belongs. It
/// must be a whole argument: a job value spliced into operator text would
/// arrive with two provenances and one string to record them in.
const JOB_PLACEHOLDER: &str = "{job}";

/// How the child's environment is set, recorded verbatim on every invocation.
const ENVIRONMENT_POLICY: &str = "cleared: the child is spawned with an empty environment, so it \
                                  cannot read the operator's provider credentials";

/// The operator's catalogue of skills this machine may invoke.
///
/// Operator configuration, under the same discipline as `corpus.json`:
/// unknown fields are load errors, because a misspelled `entrypoint` is a
/// declaration lost, and the recovery — falling back to some other entrypoint
/// — would execute code the operator did not name.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SkillCatalogue {
    /// Keyed by the name the operator uses for the skill, which becomes the
    /// provider name `skill:<key>`.
    pub skills: std::collections::BTreeMap<String, SkillDeclaration>,
}

/// What the operator declares about one skill. Everything the invocation
/// needs is here, because the skill itself declares none of it.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SkillDeclaration {
    /// The bundle's root directory. Everything executed must resolve inside
    /// it.
    pub root: String,
    /// The entrypoint to execute, relative to `root`.
    pub entrypoint: String,
    /// The interpreter to execute it with, as an absolute path. Absolute
    /// because a name resolved through `PATH` is decided by the environment
    /// this adapter has just emptied, and the record must name the program
    /// that actually ran.
    pub interpreter: String,
    /// The arguments after the entrypoint. Each is operator-declared text, or
    /// exactly `{job}` for the job's own value.
    pub arguments: Vec<String>,
    /// Wall-clock bound. Exceeding it kills the child and refuses the result;
    /// there is no default, because an execution limit nobody declared is not
    /// a limit.
    pub timeout_ms: u64,
    /// The most this runtime will accept from either stream. A result past it
    /// is refused whole.
    pub maximum_output_bytes: u64,
}

impl SkillCatalogue {
    /// Read a catalogue, rejecting anything that could not be honestly
    /// executed or recorded before a single child is spawned.
    pub fn load(path: &Path) -> Result<Self, SupplyError> {
        let encoded = std::fs::read(path).map_err(|error| SupplyError::Transport {
            detail: format!(
                "cannot read the skill catalogue {}: {error}",
                path.display()
            ),
        })?;
        let catalogue: Self =
            serde_json::from_slice(&encoded).map_err(|error| SupplyError::Malformed {
                detail: format!("{} is not a valid skill catalogue: {error}", path.display()),
            })?;
        for (name, declaration) in &catalogue.skills {
            declaration
                .validate(name)
                .map_err(|detail| SupplyError::Malformed {
                    detail: format!("{}: {detail}", path.display()),
                })?;
        }
        Ok(catalogue)
    }
}

impl SkillDeclaration {
    /// The checks that need no filesystem: they are about the declaration
    /// itself, so they fail at load rather than in the middle of a run.
    /// Containment and the digests are resolved at invocation instead, back
    /// to back with the execution, so nothing can be swapped between the
    /// check and the spawn.
    fn validate(&self, name: &str) -> Result<(), String> {
        if !Path::new(&self.interpreter).is_absolute() {
            return Err(format!(
                "skill {name} declares interpreter {:?}, which is not an absolute path; a name \
                 resolved through PATH would be decided by an environment this adapter clears",
                self.interpreter
            ));
        }
        if self.entrypoint.trim().is_empty() || self.root.trim().is_empty() {
            return Err(format!("skill {name} declares an empty root or entrypoint"));
        }
        for argument in &self.arguments {
            if argument.contains(JOB_PLACEHOLDER) && argument != JOB_PLACEHOLDER {
                return Err(format!(
                    "skill {name} declares argument {argument:?}, which embeds {JOB_PLACEHOLDER} \
                     in operator text; a job value must be its own argument so the record can say \
                     which parts of the command line came from the job and which from the \
                     operator"
                ));
            }
        }
        if self.timeout_ms == 0 || self.maximum_output_bytes == 0 {
            return Err(format!(
                "skill {name} declares a zero timeout_ms or maximum_output_bytes, which no \
                 invocation could ever satisfy"
            ));
        }
        Ok(())
    }
}

/// Where one argument of the executed command line came from. Recorded per
/// argument, because "the operator declared this" and "the job supplied this"
/// are different provenances and a single verbatim argv cannot tell them
/// apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArgumentSource {
    /// Declared in the operator's catalogue: the interpreter, the resolved
    /// entrypoint and every literal argument.
    Operator,
    /// The job's own value, substituted for `{job}`.
    Job,
}

/// One execution, as the run records it.
///
/// Everything here is observed at the boundary this runtime controls. It is
/// the answer to "what actually ran, under whose declaration, and what came
/// back" — the skill-supply counterpart of a sealed HTTP exchange, and the
/// only place a reader can check that the bytes admitted as supply were
/// produced by the bundle the catalogue names.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invocation {
    /// The name the bundle declares for itself in `SKILL.md`. The supplier's
    /// own claim, recorded beside the operator's key rather than merged with
    /// it: when they disagree, a reader should see both.
    pub declared_name: Option<String>,
    /// The operator's key for this skill, which is also its provider name
    /// without the prefix.
    pub catalogue_name: String,
    /// Always null. The Agent Skill format declares no version key — its
    /// validator's frontmatter allow-list is `name`, `description`,
    /// `license`, `allowed-tools`, `metadata` — so there is no version to
    /// record, and identity rests on the two digests below. Present as a
    /// field so the absence is visible in the artefact rather than only in
    /// prose (`docs/FAIL-POLICY.md` §7).
    pub declared_version: Option<String>,
    /// The licence the bundle declares in `SKILL.md`, verbatim. It licenses
    /// the *procedure*; what may be done with the result is unstated
    /// anywhere, which is why the envelope's licence stays unknown.
    pub declared_licence: Option<String>,
    /// The catalogue that declared this invocation.
    pub catalogue: String,
    pub root: String,
    pub skill_md_sha256: String,
    /// The entrypoint as resolved, and the digest of the bytes read
    /// immediately before executing them.
    pub entrypoint: String,
    pub entrypoint_sha256: String,
    pub interpreter: String,
    pub cwd: String,
    /// The command line as executed, verbatim, and one provenance per
    /// element.
    pub argv: Vec<String>,
    pub argv_source: Vec<ArgumentSource>,
    pub environment: String,
    /// The child's exit code, or null where a signal ended it — in which case
    /// `termination` says so rather than a code standing in for one.
    pub exit_code: Option<i32>,
    pub termination: String,
    pub stdout_bytes: u64,
    pub stdout_sha256: String,
    pub stderr_bytes: u64,
    pub stderr_sha256: String,
    pub duration_ms: u64,
    /// The limits this invocation ran under, as declared.
    pub timeout_ms: u64,
    pub maximum_output_bytes: u64,
}

/// A skill on this machine, invoked through the operator's declaration.
pub struct LocalSkillAdapter {
    provider: String,
    catalogue_name: String,
    catalogue_path: PathBuf,
    declaration: SkillDeclaration,
}

impl LocalSkillAdapter {
    /// Build the adapter for `name` from a loaded catalogue.
    pub fn new(catalogue_path: &Path, name: &str, declaration: SkillDeclaration) -> Self {
        Self {
            provider: format!("{SKILL_PROVIDER_PREFIX}{name}"),
            catalogue_name: name.to_owned(),
            catalogue_path: catalogue_path.to_path_buf(),
            declaration: declaration.clone(),
        }
    }
}

impl SupplyAdapter for LocalSkillAdapter {
    fn provider(&self) -> &str {
        &self.provider
    }

    fn capabilities(&self) -> &'static [ProviderCapability] {
        CAPABILITIES
    }

    /// Execute the declared entrypoint once and return what it produced.
    ///
    /// The order is the contract: resolve inside the root, read and hash the
    /// bytes about to run, then spawn them. Nothing is executed before the
    /// containment check, and the digest is of the bytes this runtime just
    /// read — the closest a supervised child can come to identifying what
    /// ran.
    fn invoke(&self, input: &str) -> Result<Acquisition, SupplyError> {
        let started = Instant::now();
        let root = Path::new(&self.declaration.root)
            .canonicalize()
            .map_err(|error| SupplyError::Transport {
                detail: format!(
                    "the skill root {} is not readable: {error}",
                    self.declaration.root
                ),
            })?;

        // A bundle with no SKILL.md declares no identity. Executing it would
        // put a result into evidence under a name only the operator's
        // catalogue asserts, with nothing from the supplier to record.
        let manifest_path = root.join("SKILL.md");
        let manifest = read_within(&root, &manifest_path, "SKILL.md")?;
        let declared = declared_identity(&manifest);

        let entrypoint_path = root.join(&self.declaration.entrypoint);
        let entrypoint = crate::resolve_within(&root, &entrypoint_path).map_err(|fault| {
            SupplyError::Malformed {
                detail: match fault {
                    Containment::Unresolvable(error) => format!(
                        "the declared entrypoint {} cannot be resolved and was not executed: \
                         {error}",
                        entrypoint_path.display()
                    ),
                    Containment::Outside => format!(
                        "the declared entrypoint {} resolves outside the skill root {} and was \
                         not executed",
                        entrypoint_path.display(),
                        root.display()
                    ),
                },
            }
        })?;
        let entrypoint_bytes =
            std::fs::read(&entrypoint).map_err(|error| SupplyError::Transport {
                detail: format!(
                    "the declared entrypoint {} could not be read and was not executed: {error}",
                    entrypoint.display()
                ),
            })?;

        let cwd = std::env::current_dir().map_err(|error| SupplyError::Transport {
            detail: format!("the working directory could not be resolved: {error}"),
        })?;

        let mut argv = vec![entrypoint.display().to_string()];
        let mut argv_source = vec![ArgumentSource::Operator];
        for argument in &self.declaration.arguments {
            if argument == JOB_PLACEHOLDER {
                argv.push(input.to_owned());
                argv_source.push(ArgumentSource::Job);
            } else {
                argv.push(argument.clone());
                argv_source.push(ArgumentSource::Operator);
            }
        }

        let outcome = execute(
            Path::new(&self.declaration.interpreter),
            &argv,
            &cwd,
            Duration::from_millis(self.declaration.timeout_ms),
            self.declaration.maximum_output_bytes,
        )?;

        let latency_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
        let invocation = Invocation {
            declared_name: declared.name.clone(),
            catalogue_name: self.catalogue_name.clone(),
            declared_version: None,
            declared_licence: declared.licence,
            catalogue: self.catalogue_path.display().to_string(),
            root: file_url(&root),
            skill_md_sha256: sha256_digest(&manifest),
            entrypoint: entrypoint.display().to_string(),
            entrypoint_sha256: sha256_digest(&entrypoint_bytes),
            interpreter: self.declaration.interpreter.clone(),
            cwd: cwd.display().to_string(),
            // The interpreter leads the executed command line, as it did in
            // the process this runtime spawned.
            argv: std::iter::once(self.declaration.interpreter.clone())
                .chain(argv)
                .collect(),
            argv_source: std::iter::once(ArgumentSource::Operator)
                .chain(argv_source)
                .collect(),
            environment: ENVIRONMENT_POLICY.to_owned(),
            exit_code: outcome.exit_code,
            termination: outcome.termination.to_owned(),
            stdout_bytes: outcome.stdout.len() as u64,
            stdout_sha256: sha256_digest(&outcome.stdout),
            stderr_bytes: outcome.stderr.len() as u64,
            stderr_sha256: sha256_digest(&outcome.stderr),
            duration_ms: outcome.duration_ms,
            timeout_ms: self.declaration.timeout_ms,
            maximum_output_bytes: self.declaration.maximum_output_bytes,
        };

        // The response this adapter answers with, sealed verbatim beside the
        // run: the streams as text where they are text, the invocation
        // record, and the one result the envelope below is derived from.
        let response = json!({
            "provider": self.provider,
            "input": input,
            "invocation": invocation,
            "stdout": String::from_utf8(outcome.stdout.clone()).ok(),
            "stderr": String::from_utf8(outcome.stderr.clone()).ok(),
            "result": result_value(&self.provider, &root, &declared.name, &outcome),
        });
        let raw_response =
            serde_json::to_vec_pretty(&response).map_err(|error| SupplyError::Malformed {
                detail: format!("could not serialise the invocation response: {error}"),
            })?;

        Ok(Acquisition {
            provider: self.provider.clone(),
            capability: ProviderCapability::Invoke,
            endpoint: file_url(&root),
            // Nothing crossed HTTP. A fabricated 200 would record a response
            // nobody sent, and an exit code is not a status code.
            http_status: None,
            latency_ms,
            provider_request_id: None,
            // No money moved and nobody priced this: unknown, never zero. A
            // local execution has no supplier to quote it, so there is no
            // purchase decision to reuse either.
            charge: AcquisitionCharge::default(),
            envelopes: envelopes_from(&response)?,
            raw_response,
            invocation: Some(invocation_of(&response)?),
        })
    }
}

/// The one result of one invocation, as the sealed response carries it.
///
/// Standard output alone is the result. A skill that writes its answer to
/// standard error produces no admissible supply and the record says why: the
/// error stream is where a program says what went wrong, and admitting it as
/// content would put diagnostics into the model's window wearing the same
/// clothes as an answer. `stderr` is sealed, sized and hashed either way.
fn result_value(
    provider: &str,
    root: &Path,
    declared_name: &Option<String>,
    outcome: &Outcome,
) -> Value {
    let text = String::from_utf8(outcome.stdout.clone()).ok();
    json!({
        "url": file_url(root),
        "title": declared_name.clone().unwrap_or_else(|| provider.to_owned()),
        "text": text,
        "content_hash": text.as_deref().map(|text| sha256_digest(text.as_bytes())),
        "stream": "stdout",
        "exit_code": outcome.exit_code,
        "termination": outcome.termination,
    })
}

/// The envelope, derived from the sealed response exactly as every other
/// adapter derives its envelopes from a provider's body.
fn envelopes_from(response: &Value) -> Result<Vec<ContextEnvelope>, SupplyError> {
    let result = &response["result"];
    let Some(url) = result["url"].as_str() else {
        return Err(SupplyError::Malformed {
            detail: "the sealed invocation response carries no result URL".to_owned(),
        });
    };
    Ok(vec![ContextEnvelope {
        // A `file://` URL has no host; the empty host is the true value.
        host: String::new(),
        source_url: url.to_owned(),
        title: result["title"].as_str().map(str::to_owned),
        text: result["text"].as_str().map(str::to_owned),
        content_hash: result["content_hash"].as_str().map(str::to_owned),
        // The skill's own `license` licenses the procedure, not the output.
        // Nothing anywhere declares what may be done with what it printed, so
        // this stays unknown — the declaration is recorded as the skill's, in
        // the invocation record, and is never promoted into a rights claim
        // over the result.
        licence: LicenceState::Unknown,
        // Nothing published a produced result, so nothing dated it. Absence
        // here is what makes a freshness-weighted objective abstain over a
        // skill plan rather than score it.
        declared_date: None,
        native_metadata: json!({
            "skill": {
                "provider": response["provider"],
                "declared_name": response["invocation"]["declared_name"],
                "entrypoint_sha256": response["invocation"]["entrypoint_sha256"],
                "exit_code": result["exit_code"],
                "termination": result["termination"],
                "stream": result["stream"],
            }
        }),
        // One invocation, one result.
        retrieval_rank: 1,
    }])
}

/// Read the invocation back from the sealed response, so the record a run
/// publishes is derived from the same document as the envelopes rather than
/// from a second copy that could drift from it.
fn invocation_of(response: &Value) -> Result<Invocation, SupplyError> {
    serde_json::from_value(response["invocation"].clone()).map_err(|error| SupplyError::Malformed {
        detail: format!("the sealed invocation record could not be read back: {error}"),
    })
}

/// What the bundle declares about itself.
struct DeclaredIdentity {
    name: Option<String>,
    licence: Option<String>,
}

/// The two frontmatter keys this runtime records, read without interpreting
/// the rest.
///
/// Deliberately not a YAML parse and deliberately not a validation: `SKILL.md`
/// is a third party's declaration, not an operator one, and this adapter's
/// business with it is to record the identity it claims. A key it cannot read
/// as a plain scalar is absent, which is the honest state — never a guess.
fn declared_identity(manifest: &[u8]) -> DeclaredIdentity {
    let text = String::from_utf8_lossy(manifest);
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return DeclaredIdentity {
            name: None,
            licence: None,
        };
    }
    let mut name = None;
    let mut licence = None;
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        // Only top-level keys: an indented line belongs to a nested block
        // (`metadata:`), and reading its `name:` as the skill's own would
        // record somebody else's field as the identity.
        if line.starts_with(char::is_whitespace) {
            continue;
        }
        if let Some(value) = line.strip_prefix("name:") {
            name = Some(value.trim().trim_matches(['"', '\'']).to_owned());
        } else if let Some(value) = line.strip_prefix("license:") {
            licence = Some(value.trim().trim_matches(['"', '\'']).to_owned());
        }
    }
    DeclaredIdentity { name, licence }
}

/// Read a file that must resolve inside the declared root.
fn read_within(root: &Path, path: &Path, what: &str) -> Result<Vec<u8>, SupplyError> {
    let resolved = crate::resolve_within(root, path).map_err(|fault| SupplyError::Malformed {
        detail: match fault {
            Containment::Unresolvable(error) => format!(
                "the skill bundle at {} declares no readable {what} ({error}), so it claims no \
                 identity of its own and nothing was executed",
                root.display()
            ),
            Containment::Outside => format!(
                "the skill bundle's {what} resolves outside {} and was not read, so nothing was \
                 executed",
                root.display()
            ),
        },
    })?;
    std::fs::read(&resolved).map_err(|error| SupplyError::Transport {
        detail: format!("cannot read {}: {error}", resolved.display()),
    })
}

/// What one child produced.
struct Outcome {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit_code: Option<i32>,
    termination: &'static str,
    /// Wall clock around the child alone, measured from the spawn. The
    /// acquisition's own latency covers the whole leg — resolution, digests
    /// and this — and the two are kept apart so a reader can tell how much of
    /// a slow plan was the skill.
    duration_ms: u64,
}

/// Spawn the declared program, bound it, and collect what it wrote.
///
/// The controls are all here and all observable: no shell (the argument
/// vector is passed as a vector), an empty environment, no standard input to
/// block on, a wall clock enforced by killing the child, and a byte cap that
/// refuses a result rather than trimming one. Unlike the in-process
/// processors, a supervised child can honestly declare a limit and enforce
/// it, which is why these numbers are in the record.
fn execute(
    interpreter: &Path,
    argv: &[String],
    cwd: &Path,
    timeout: Duration,
    maximum_output_bytes: u64,
) -> Result<Outcome, SupplyError> {
    let spawned = Instant::now();
    let mut command = std::process::Command::new(interpreter);
    command
        .args(argv)
        .current_dir(cwd)
        // Emptied, not filtered: an allow-list of variables to keep is a list
        // that grows, and every credential this operator holds is reachable
        // from the environment this process was started with.
        .env_clear()
        // A skill that reads standard input gets end-of-file at once. An
        // inherited stdin would let one block for ever behind a prompt
        // nobody is there to answer.
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // The child leads its own process group, so the timeout can signal
    // everything the invocation started rather than only the process this
    // runtime holds a handle to. Process groups are a Unix construct; on
    // Windows the timeout reaches only the direct child (see `kill_group`).
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(|error| SupplyError::Transport {
        detail: format!(
            "the declared interpreter {} could not be executed: {error}",
            interpreter.display()
        ),
    })?;

    let exceeded = Arc::new(AtomicBool::new(false));
    let cap = maximum_output_bytes as usize;
    // Both streams are drained on their own threads for the whole life of the
    // child. Reading them in turn on this one would deadlock the moment a
    // program filled the pipe this runtime was not currently reading, and a
    // skill that logs while it works is an ordinary program.
    let out_stream = child.stdout.take();
    let err_stream = child.stderr.take();
    let out_reader = out_stream.map(|stream| {
        let exceeded = Arc::clone(&exceeded);
        std::thread::spawn(move || read_capped(stream, cap, &exceeded))
    });
    let err_reader = err_stream.map(|stream| {
        let exceeded = Arc::clone(&exceeded);
        std::thread::spawn(move || read_capped(stream, cap, &exceeded))
    });

    // Poll rather than block: the child must be killed both when the clock
    // runs out and the moment a stream passes the cap, and a blocking wait
    // could not do the second at all — a child whose pipe this runtime has
    // stopped draining would sit in a write until the timeout, turning an
    // oversized result into a slow one.
    let deadline = Instant::now() + timeout;
    let mut status = None;
    let mut over_cap = false;
    loop {
        match child.try_wait() {
            Ok(Some(exit)) => {
                status = Some(exit);
                break;
            }
            Ok(None) => {}
            Err(error) => {
                kill_group(&mut child);
                return Err(SupplyError::Transport {
                    detail: format!("the invoked child could not be waited on: {error}"),
                });
            }
        }
        if exceeded.load(Ordering::Relaxed) {
            over_cap = true;
            kill_group(&mut child);
            break;
        }
        if Instant::now() >= deadline {
            kill_group(&mut child);
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    let duration_ms = spawned.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    let joined = |reader: Option<std::thread::JoinHandle<Vec<u8>>>| {
        reader
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default()
    };
    let stdout = joined(out_reader);
    let stderr = joined(err_reader);

    if over_cap || exceeded.load(Ordering::Relaxed) {
        return Err(SupplyError::Execution {
            detail: format!(
                "the invocation wrote more than the declared {maximum_output_bytes}-byte cap, so \
                 its result is refused whole; a truncated result is a different result, and its \
                 hash would match nothing"
            ),
        });
    }
    let Some(status) = status else {
        return Err(SupplyError::Execution {
            detail: format!(
                "the invocation did not finish within the declared {} ms and was killed; no \
                 partial result is offered",
                timeout.as_millis()
            ),
        });
    };

    Ok(Outcome {
        stdout,
        stderr,
        duration_ms,
        exit_code: status.code(),
        // A child killed by a signal has no exit code, and recording one
        // would invent a verdict the program never gave.
        termination: if status.code().is_some() {
            "exited"
        } else {
            "signalled"
        },
    })
}

/// End the invocation: signal the child's whole process group, then reap it.
///
/// The group, not the process. `Child::kill` signals the one process this
/// runtime holds a handle to, and a skill that spawned a helper — a shell
/// running `sleep`, a validator shelling out — leaves that helper alive and
/// still holding the output pipe. The reader threads would then wait on the
/// grandchild, and a declared timeout that waits for the process it just
/// killed is not a limit at all. Observed exactly that way: a 200 ms bound
/// took thirty seconds to return
/// (`tests/skill_invocation.rs::an_invocation_over_its_declared_timeout_is_killed_and_refused`).
///
/// Anything the group leaves behind — a process that escaped into its own
/// group, a descendant that outlived the signal — is beyond what a supervised
/// spawn can promise. No probe has yet tried to escape the group, so how far
/// the signal reaches in that case is unmeasured.
fn kill_group(child: &mut std::process::Child) {
    // Negative pid means the process group, which the spawn established as
    // the child's own. Failure is not actionable here — the group is gone, or
    // it is not ours — and the caller refuses the result either way.
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL)
    };
    // On Windows there is no process group to signal: only the direct child
    // is killed, and a helper it spawned may outlive the timeout. The bound
    // still refuses the result; it cannot promise the machine is quiet.
    let _ = child.kill();
    let _ = child.wait();
}

/// Read one of the child's streams until it ends or passes the cap, flagging
/// the caller the moment it does so the child can be killed rather than left
/// writing into a pipe nobody drains.
///
/// A read error ends the collection rather than failing the invocation: the
/// child's exit status and the bytes already taken are still the record, and
/// the caller's cap and timeout checks still decide what that record is worth.
fn read_capped(mut stream: impl Read, cap: usize, exceeded: &AtomicBool) -> Vec<u8> {
    let mut collected = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return collected,
            Ok(read) => collected.extend_from_slice(&chunk[..read]),
        }
        if collected.len() > cap {
            exceeded.store(true, Ordering::Relaxed);
            return collected;
        }
    }
}

fn file_url(path: &Path) -> String {
    url::Url::from_file_path(path)
        .map(String::from)
        .unwrap_or_else(|_| format!("file://{}", path.display()))
}
