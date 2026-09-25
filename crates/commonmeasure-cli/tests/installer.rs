//! The installer, run as a person runs it: `sh install.sh` against a loopback
//! origin standing where the public release stands. The origin answers with
//! the URL shape of a GitHub release: `releases/latest` redirects to the
//! newest release's tag page, and `releases/download/<tag>/<asset>` redirects
//! to the bytes, which are this build's own binary for the platform asset and
//! a checksum list naming every asset a release holds. No credential is sent
//! or needed. The checksum, version and missing-binary refusals are driven
//! with no network; the release itself is exercised by the release workflow's
//! verify job and the container check `RELEASING.md` records.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use commonmeasure_http::{Request, Response, Server, ServerHandle};
use sha2::{Digest, Sha256};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const REPOSITORY: &str = "commonmeasure/commonmeasure";

/// Every asset a release holds besides SHA256SUMS, as the workflow names them.
const ASSETS: &[&str] = &[
    "commonmeasure-linux-x64",
    "commonmeasure-linux-arm64",
    "commonmeasure-win-x64.exe",
    "commonmeasure-darwin-arm64",
    "commonmeasure-darwin-x64",
    "install.sh",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/commonmeasure-cli sits two levels below the repository root")
        .to_path_buf()
}

/// The asset name the installer picks on the platform the test runs on,
/// which is what the launcher and the release call that platform's binary.
fn asset_for_this_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "commonmeasure-linux-x64",
        ("linux", "aarch64") => "commonmeasure-linux-arm64",
        ("macos", "aarch64") => "commonmeasure-darwin-arm64",
        ("macos", "x86_64") => "commonmeasure-darwin-x64",
        other => panic!("the release has no binary for {other:?}"),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn redirect(to: &str) -> Response {
    let mut response = Response::new(302, Vec::new());
    response.headers.set("Location", to);
    response
}

/// A release at `tag` whose platform binary is `binary` and whose checksum
/// list says `listed_sum` for it. With `with_this_platform` false the release
/// holds no binary for the platform the test runs on, and its checksum list
/// says so by omission. Every other asset is listed with an arbitrary sum,
/// because the installer downloads only the two it needs.
fn release(tag: &str, binary: Vec<u8>, listed_sum: &str, with_this_platform: bool) -> ServerHandle {
    let asset = asset_for_this_platform();
    let version = tag.trim_start_matches('v');
    let mut names: Vec<String> = ASSETS.iter().map(|name| (*name).to_owned()).collect();
    names.push(format!("commonmeasure-plugin-{version}.tar.gz"));
    let mut sums = String::new();
    for name in names {
        if name == asset {
            if with_this_platform {
                sums.push_str(&format!("{listed_sum}  {name}\n"));
            }
        } else {
            sums.push_str(&format!("{}  {name}\n", sha256_hex(name.as_bytes())));
        }
    }
    let server = Server::bind("127.0.0.1:0").expect("bind");
    let origin = format!("http://{}", server.local_addr().expect("local addr"));
    let releases = format!("/{REPOSITORY}/releases");
    let tag = tag.to_owned();
    server
        .spawn(move |request: Request| {
            let path = request.target.split('?').next().unwrap_or_default();
            if path == format!("{releases}/latest") {
                redirect(&format!("{origin}{releases}/tag/{tag}"))
            } else if let Some(rest) = path.strip_prefix(&format!("{releases}/download/")) {
                redirect(&format!("{origin}/objects/{rest}"))
            } else if path == format!("/objects/{tag}/SHA256SUMS") {
                Response::new(200, sums.clone().into_bytes())
            } else if path == format!("/objects/{tag}/{asset}") {
                Response::new(200, binary.clone())
            } else {
                Response::text(404, "no such route")
            }
        })
        .expect("spawn")
}

fn install(origin: &ServerHandle, home: &Path, dir: &Path, tag: Option<&str>) -> Output {
    let mut command = Command::new("/bin/sh");
    command.arg(repo_root().join("install.sh"));
    if let Some(tag) = tag {
        command.args(["--tag", tag]);
    }
    command
        .arg("--dir")
        .arg(dir)
        .env("HOME", home)
        .env(
            "COMMONMEASURE_RELEASE_URL",
            format!("{}/{REPOSITORY}/releases", origin.url()),
        )
        .output()
        .expect("sh runs the installer")
}

fn this_binary() -> Vec<u8> {
    std::fs::read(env!("CARGO_BIN_EXE_commonmeasure")).expect("the built binary is readable")
}

fn assert_installed(output: &Output, dir: &Path) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(stdout.contains("checksum verified"), "{stdout}");
    assert!(
        stdout.contains("is not on PATH"),
        "the installer says when it cannot put the directory on PATH: {stdout}"
    );

    let reported = Command::new(dir.join("commonmeasure"))
        .arg("--version")
        .output()
        .expect("the installed binary runs");
    assert_eq!(
        String::from_utf8_lossy(&reported.stdout).trim(),
        format!("commonmeasure {VERSION}")
    );
}

#[test]
fn installs_the_verified_binary_and_reports_the_release_version() {
    let binary = this_binary();
    let sum = sha256_hex(&binary);
    let tag = format!("v{VERSION}");
    let origin = release(&tag, binary, &sum, true);
    let home = tempfile::tempdir().expect("tempdir");
    let dir = home.path().join("bin");

    let output = install(&origin, home.path(), &dir, Some(&tag));
    assert_installed(&output, &dir);
}

#[test]
fn installs_the_latest_release_when_no_tag_is_given() {
    let binary = this_binary();
    let sum = sha256_hex(&binary);
    let tag = format!("v{VERSION}");
    let origin = release(&tag, binary, &sum, true);
    let home = tempfile::tempdir().expect("tempdir");
    let dir = home.path().join("bin");

    let output = install(&origin, home.path(), &dir, None);
    assert_installed(&output, &dir);
}

#[test]
fn refuses_a_binary_whose_checksum_does_not_match() {
    let binary = this_binary();
    let wrong = sha256_hex(b"not these bytes");
    let tag = format!("v{VERSION}");
    let origin = release(&tag, binary, &wrong, true);
    let home = tempfile::tempdir().expect("tempdir");
    let dir = home.path().join("bin");

    let output = install(&origin, home.path(), &dir, Some(&tag));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("does not match its SHA-256"), "{stderr}");
    assert!(
        !dir.join("commonmeasure").exists(),
        "nothing is placed on PATH before the checksum is verified"
    );
}

#[test]
fn removes_a_binary_that_reports_another_version_than_the_release() {
    let binary = this_binary();
    let sum = sha256_hex(&binary);
    let origin = release("v9.9.9", binary, &sum, true);
    let home = tempfile::tempdir().expect("tempdir");
    let dir = home.path().join("bin");

    let output = install(&origin, home.path(), &dir, Some("v9.9.9"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("but the release is v9.9.9"), "{stderr}");
    assert!(
        !dir.join("commonmeasure").exists(),
        "the binary was removed"
    );
}

#[test]
fn names_a_platform_the_release_has_no_binary_for_and_lists_what_it_holds() {
    let binary = this_binary();
    let sum = sha256_hex(&binary);
    let tag = format!("v{VERSION}");
    let origin = release(&tag, binary, &sum, false);
    let home = tempfile::tempdir().expect("tempdir");
    let dir = home.path().join("bin");

    let output = install(&origin, home.path(), &dir, Some(&tag));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains(&format!("has no {}", asset_for_this_platform())),
        "{stderr}"
    );
    assert!(
        stderr.contains("commonmeasure-win-x64.exe"),
        "the binaries the release holds are listed: {stderr}"
    );
    assert!(
        !stderr.contains(".tar.gz"),
        "the plugin archive is not a binary: {stderr}"
    );
    assert!(!dir.exists(), "nothing is installed");
}

#[test]
fn names_a_release_that_does_not_exist() {
    let binary = this_binary();
    let sum = sha256_hex(&binary);
    let origin = release(&format!("v{VERSION}"), binary, &sum, true);
    let home = tempfile::tempdir().expect("tempdir");
    let dir = home.path().join("bin");

    let output = install(&origin, home.path(), &dir, Some("v0.0.0"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("cannot read release v0.0.0"), "{stderr}");
    assert!(!dir.exists(), "nothing is installed");
}

/// A directory holding only the named tools, so the installer sees a machine
/// without the others.
fn path_with(tools: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for tool in tools {
        let found = std::env::var_os("PATH")
            .and_then(|path| {
                std::env::split_paths(&path)
                    .map(|p| p.join(tool))
                    .find(|p| p.is_file())
            })
            .unwrap_or_else(|| panic!("{tool} is on PATH"));
        std::os::unix::fs::symlink(found, dir.path().join(tool)).expect("symlink");
    }
    dir
}

#[test]
fn names_curl_when_it_is_missing() {
    let path = path_with(&[]);
    let home = tempfile::tempdir().expect("tempdir");
    let output = Command::new("/bin/sh")
        .arg(repo_root().join("install.sh"))
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", path.path())
        .output()
        .expect("sh runs the installer");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("curl is not installed"), "{stderr}");
}

#[test]
fn needs_no_credential_and_no_github_client() {
    // The installer runs with an environment holding nothing but HOME, the
    // release location and the system's own PATH, where curl and a checksum
    // tool are and gh is not.
    let binary = this_binary();
    let sum = sha256_hex(&binary);
    let tag = format!("v{VERSION}");
    let origin = release(&tag, binary, &sum, true);
    let home = tempfile::tempdir().expect("tempdir");
    let dir = home.path().join("bin");
    let output = Command::new("/bin/sh")
        .arg(repo_root().join("install.sh"))
        .args(["--tag", &tag, "--dir"])
        .arg(&dir)
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .env(
            "COMMONMEASURE_RELEASE_URL",
            format!("{}/{REPOSITORY}/releases", origin.url()),
        )
        .output()
        .expect("sh runs the installer");
    assert_installed(&output, &dir);
}
