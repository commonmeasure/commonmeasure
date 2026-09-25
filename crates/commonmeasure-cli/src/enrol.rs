use commonmeasure_harness::{directory, home_dir};
use std::path::PathBuf;

#[derive(clap::Args)]
pub struct Enrol {
    /// Project directory; defaults to the CLI working directory.
    #[arg(long)]
    directory: Option<PathBuf>,
    /// Project name. Required when changing enrolment.
    #[arg(long)]
    name: Option<String>,
    /// Choose local recording or permitted hub reporting.
    #[arg(long, value_parser = ["local", "hub"])]
    reporting: Option<String>,
    /// Acknowledge that hub reporting includes existing eligible evidence.
    #[arg(long)]
    include_history: bool,
    /// Remove local reporting permission, preserving evidence and edge keys.
    #[arg(long, conflicts_with_all = ["name", "reporting", "include_history"])]
    remove: bool,
    /// Refresh the existing managed source policy and signed reporting approvals.
    #[arg(long)]
    sync: bool,
}

pub fn run(args: Enrol) -> Result<(), String> {
    let home = home_dir().map_err(|e| e.to_string())?;
    let root = directory::selected(
        &args
            .directory
            .unwrap_or(std::env::current_dir().map_err(|e| e.to_string())?),
    )?;
    if args.remove {
        let registry = directory::Registry::read(&home)?.ok_or("directory is not enrolled")?;
        let old = registry
            .matching(root.to_str().ok_or("non-UTF-8 directory")?)
            .ok_or("directory is not enrolled")?;
        if old.root != root {
            return Err(format!(
                "remove reporting at its enrolled root: {}",
                old.root.display()
            ));
        }
        let project = directory::Registry::enrol(&home, &root, &old.name, false)?;
        if commonmeasure_harness::managed::is_managed(&home)?
            && let Err(error) = directory::request(&home, &project)
        {
            eprintln!("local reporting removed; hub update pending: {error}");
        }
    } else if args.name.is_some() || args.reporting.is_some() {
        let name = args.name.ok_or("provide --name <project-name>")?;
        let reporting = args
            .reporting
            .ok_or("choose --reporting local or --reporting hub")?
            == "hub";
        if reporting && !args.include_history {
            return Err(format!(
                "hub reporting covers {} and descendants, including existing eligible witnessed evidence. Confirm with --include-history; related Git worktrees need separate enrolment",
                root.display()
            ));
        }
        // Validate managed authority before writing the local selection.
        commonmeasure_harness::policy::PolicyDocument::read(&home)?;
        let project = directory::Registry::enrol(&home, &root, &name, reporting)?;
        if reporting && commonmeasure_harness::managed::is_managed(&home)? {
            let result = directory::request(&home, &project)?;
            println!(
                "hub request: {}",
                serde_json::to_string(&result).map_err(|e| e.to_string())?
            );
            directory::sync(&home)?;
        }
    }
    if args.sync {
        directory::sync_all(&home)?;
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&directory::status(&home, &root)?)
            .map_err(|e| e.to_string())?
    );
    Ok(())
}
