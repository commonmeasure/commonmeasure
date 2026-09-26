//! One version identity: the workspace manifest is the source, the binary
//! reports it, and the plugin and browser extension manifests carry the same
//! string.
//!
//! Each manifest is a committed JSON file its host reads (Claude Code, the
//! browser), so neither can take its version from the build. These tests are
//! what bind them to the workspace; the release workflow's version job runs
//! this whole file before building anything.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The version every workspace crate compiles with (`version.workspace`).
const WORKSPACE_VERSION: &str = env!("CARGO_PKG_VERSION");

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/commonmeasure-cli sits two levels below the repository root")
        .to_path_buf()
}

#[test]
fn the_plugin_manifest_carries_the_workspace_version() {
    let manifest = repo_root().join("plugin/.claude-plugin/plugin.json");
    let text = std::fs::read_to_string(&manifest).expect("plugin.json is readable");
    let json: serde_json::Value = serde_json::from_str(&text).expect("plugin.json parses");
    assert_eq!(
        json["version"].as_str(),
        Some(WORKSPACE_VERSION),
        "plugin/.claude-plugin/plugin.json must carry the workspace version from Cargo.toml"
    );
}

#[test]
fn the_browser_manifest_carries_the_workspace_version() {
    let manifest = repo_root().join("browser/manifest.json");
    let text = std::fs::read_to_string(&manifest).expect("manifest.json is readable");
    let json: serde_json::Value = serde_json::from_str(&text).expect("manifest.json parses");
    assert_eq!(
        json["version"].as_str(),
        Some(WORKSPACE_VERSION),
        "browser/manifest.json must carry the workspace version from Cargo.toml"
    );
}

#[test]
fn the_binary_reports_the_workspace_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .arg("--version")
        .output()
        .expect("the binary runs");
    assert!(output.status.success(), "--version exits zero");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("commonmeasure {WORKSPACE_VERSION}")
    );
}
