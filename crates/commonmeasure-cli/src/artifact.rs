//! Explicit local artifact declarations and portable snapshot verification.

use std::path::PathBuf;

use clap::{Args, Subcommand};
use commonmeasure_runtime::artifact::{self, EvidenceInput, VerifyTarget};
use serde_json::{Value, json};

#[derive(Args)]
pub struct Artifact {
    #[command(subcommand)]
    command: ArtifactCommand,
}

#[derive(Subcommand)]
enum ArtifactCommand {
    /// Initialise a local record store, or read its existing stable identity.
    Init {
        #[arg(long)]
        store: PathBuf,
        /// Existing UUID URN to retain. Omit to generate a new artifact identity.
        #[arg(long)]
        artifact_id: Option<String>,
    },
    /// Record an explicit session association. Does not assert source use or authorship.
    Associate(Associate),
    /// Capture a saved file or fixed Git tree with selected local evidence.
    Snapshot {
        #[command(subcommand)]
        command: SnapshotCommand,
    },
    /// Export a snapshot and its selected records/evidence into a new JSON sidecar.
    /// Evidence can be private: choose deliberately where this file is shared.
    Export {
        #[arg(long)]
        store: PathBuf,
        #[arg(long)]
        snapshot: String,
        /// New file. Existing files are never overwritten.
        #[arg(long)]
        output: PathBuf,
    },
    /// Verify a portable JSON bundle against an explicit artifact, entirely offline.
    /// A match establishes byte integrity; declarations remain unsigned.
    Verify(Verify),
    /// Print a Markdown index of a local store. Redirect it to COMMONMEASURE.md.
    /// The index is a generated view; JSON records remain authoritative.
    Index {
        #[arg(long)]
        store: PathBuf,
    },
}

#[derive(Args)]
struct Associate {
    #[arg(long)]
    store: PathBuf,
    /// Original Edge session ID, not a guessed host task or Hub UUID.
    #[arg(long)]
    session: String,
    /// Issuer-defined session namespace (URI or URN).
    #[arg(long)]
    namespace: String,
    /// Stable Edge installation identifier; a local label is a declaration only.
    #[arg(long)]
    edge: String,
    /// Issuer of the Edge installation identifier (URI or URN).
    #[arg(long)]
    issuer: String,
    /// Claimed declarant identity. This command does not authenticate that identity.
    #[arg(long)]
    actor: String,
    #[arg(long, value_parser = ["agent_declaration", "operator_declaration"])]
    basis: String,
    #[arg(long, default_value = "unspecified", value_parser = ["research", "drafting", "review", "transformation", "unspecified"])]
    role: String,
    #[arg(long)]
    host: Option<String>,
}

#[derive(Args)]
struct Capture {
    #[arg(long)]
    store: PathBuf,
    /// Copy a session log, explicitly tied to a declaration: ASSOCIATION_UUID=PATH.
    /// Repeat for more logs. No logs are discovered or copied by default.
    #[arg(long, value_parser = evidence_input)]
    evidence: Vec<EvidenceArgument>,
}

#[derive(Clone)]
struct EvidenceArgument {
    association_id: String,
    path: PathBuf,
}

#[derive(Subcommand)]
enum SnapshotCommand {
    /// Bind every byte of the saved file. Does not modify the file or its metadata.
    File {
        file: PathBuf,
        #[command(flatten)]
        capture: Capture,
    },
    /// Bind all supported tracked blobs in a fixed Git tree, excluding provenance paths.
    /// Uncommitted, untracked and ignored work is outside this binding.
    Git {
        repo: PathBuf,
        #[arg(long, default_value = "HEAD")]
        revision: String,
        #[command(flatten)]
        capture: Capture,
    },
}

#[derive(Args)]
struct Verify {
    bundle: PathBuf,
    #[arg(long, required_unless_present = "repo", conflicts_with = "repo")]
    file: Option<PathBuf>,
    #[arg(long, required_unless_present = "file")]
    repo: Option<PathBuf>,
    /// Explicit target Git revision. Defaults to HEAD when --repo is supplied.
    #[arg(long, requires = "repo")]
    revision: Option<String>,
    /// Snapshot digest retained separately from the bundle. Detects replacement of
    /// the whole unsigned bundle; does not authenticate the original declaration.
    #[arg(long)]
    expected_snapshot: Option<String>,
}

fn evidence_input(value: &str) -> Result<EvidenceArgument, String> {
    let (association_id, path) = value
        .split_once('=')
        .filter(|(id, path)| !id.is_empty() && !path.is_empty())
        .ok_or("use --evidence ASSOCIATION_UUID=PATH")?;
    Ok(EvidenceArgument {
        association_id: association_id.to_owned(),
        path: PathBuf::from(path),
    })
}

fn evidence(args: &[EvidenceArgument]) -> Vec<EvidenceInput> {
    args.iter()
        .map(|item| EvidenceInput {
            association_id: item.association_id.clone(),
            path: item.path.clone(),
        })
        .collect()
}

pub fn run(args: Artifact) -> Result<(), String> {
    let result = match args.command {
        ArtifactCommand::Init { store, artifact_id } => {
            artifact::init_store(&store, artifact_id.as_deref())?
        }
        ArtifactCommand::Associate(args) => {
            let mut request = json!({
                "session": {
                    "namespace": args.namespace, "id": args.session,
                    "edge": {"issuer": args.issuer, "installation_id": args.edge}
                },
                "role": args.role,
                "assertion": {"basis": args.basis, "actor": args.actor}
            });
            if let Some(host) = args.host {
                request["session"]["host"] = json!(host);
            }
            artifact::associate(&args.store, &request)?
        }
        ArtifactCommand::Snapshot { command } => match command {
            SnapshotCommand::File { file, capture } => {
                artifact::snapshot_file(&capture.store, &file, &evidence(&capture.evidence))?
            }
            SnapshotCommand::Git {
                repo,
                revision,
                capture,
            } => artifact::snapshot_repo(
                &capture.store,
                &repo,
                &revision,
                &evidence(&capture.evidence),
            )?,
        },
        ArtifactCommand::Export {
            store,
            snapshot,
            output,
        } => {
            let bundle = artifact::export_bundle(&store, &snapshot)?;
            artifact::write_bundle(&output, &bundle)?;
            json!({"output": output, "snapshot_id": snapshot,
                   "snapshot_digest": bundle["snapshot_digest"]})
        }
        ArtifactCommand::Verify(args) => {
            let target = match (args.file, args.repo) {
                (Some(file), None) => VerifyTarget::File(file),
                (None, Some(repo)) => VerifyTarget::Git {
                    repo,
                    revision: args.revision.unwrap_or_else(|| "HEAD".to_owned()),
                },
                _ => return Err("choose exactly one of --file and --repo".into()),
            };
            let bundle = artifact::read_json(&args.bundle)?;
            let report =
                artifact::verify_bundle(&bundle, &target, args.expected_snapshot.as_deref())?;
            write_json(&report)?;
            return if report["valid"] == true {
                Ok(())
            } else {
                Err("artifact verification failed; see the JSON report".into())
            };
        }
        ArtifactCommand::Index { store } => {
            return super::write_stdout(&artifact::markdown_index(&store)?);
        }
    };
    write_json(&result)
}

fn write_json(value: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    super::write_stdout(&format!("{text}\n"))
}
