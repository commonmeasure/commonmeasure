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
use std::sync::{Arc, Mutex};

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
    recorded_release(tag, binary, listed_sum, with_this_platform).0
}

/// Every path requested of a release origin, in order.
type Requests = Arc<Mutex<Vec<String>>>;

/// [`release`], also returning the paths it was asked for.
fn recorded_release(
    tag: &str,
    binary: Vec<u8>,
    listed_sum: &str,
    with_this_platform: bool,
) -> (ServerHandle, Requests) {
    let requests = Requests::default();
    let log = Arc::clone(&requests);
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
    let server = server
        .spawn(move |request: Request| {
            let path = request.target.split('?').next().unwrap_or_default();
            log.lock().unwrap().push(path.to_owned());
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
        .expect("spawn");
    (server, requests)
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
fn places_nothing_when_the_binary_reports_another_version_than_the_release() {
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
        "the version is checked before the binary is placed"
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

// `commonmeasure update` runs the installer compiled into the binary against
// the same loopback release. The binary under test is copied into a
// directory of its own and run from there, so the file it replaces is that
// copy. HOME is empty, so there is no plist to read. On macOS launchd is
// asked, read-only, whether the service is loaded, under a scratch label
// (`COMMONMEASURE_SERVICE_LABEL`, which a debug build takes): no test
// addresses the machine owner's `ai.commonmeasure.console`, even to read it.
// A `launchctl` first on PATH records each call and forwards to the real one
// only `print gui/<uid>/<scratch label>`, exactly those two arguments; it
// refuses any other call before forwarding, and a refused call fails the
// test. No test needs `bootstrap` or `bootout`: the scratch label is never
// loaded, so `update` finds no service to stop or start.
//
// These tests run only in a debug build: a release build ignores
// `COMMONMEASURE_SERVICE_LABEL` and `COMMONMEASURE_RELEASE_URL` by design,
// so under `cargo test --release` the copied binary would ask launchd about
// the owner's label and fetch from the public repository.

/// A release binary that is a script reporting `version`: enough for the
/// installer's version check, and distinguishable from the binary it
/// replaces.
#[cfg(debug_assertions)]
fn script_reporting(version: &str) -> Vec<u8> {
    format!("#!/bin/sh\necho \"commonmeasure {version}\"\n").into_bytes()
}

/// This build's binary, copied to `<home>/bin/commonmeasure`.
#[cfg(debug_assertions)]
fn installed_copy(home: &Path) -> PathBuf {
    let dir = home.join("bin");
    std::fs::create_dir_all(&dir).expect("bin");
    let target = dir.join("commonmeasure");
    std::fs::copy(env!("CARGO_BIN_EXE_commonmeasure"), &target).expect("copy the binary");
    target
}

/// The `launchctl` the update tests put first on PATH. It logs each call,
/// then forwards to `/bin/launchctl` only `print gui/<uid>/<label>`; any other
/// operation, target or argument count is logged as `REFUSED` and exits 97
/// without forwarding. Checking the whole call, not each argument, is what
/// keeps a domain-wide `print gui/<uid>` or `bootout gui/<uid>` from reaching
/// launchd.
fn launchctl_wrapper(label: &str, log: &Path) -> String {
    format!(
        "#!/bin/sh\n\
         log='{log}'\n\
         printf '%s\\n' \"$*\" >> \"$log\"\n\
         if [ \"$#\" -ne 2 ] || [ \"$1\" != print ] || [ \"$2\" != \"gui/$(id -u)/{label}\" ]; then\n\
         \tprintf 'REFUSED %s\\n' \"$*\" >> \"$log\"\n\
         \texit 97\n\
         fi\n\
         exec /bin/launchctl \"$@\"\n",
        log = log.display()
    )
}

/// Runs [`launchctl_wrapper`] with its final `exec` replaced by a marker, so
/// nothing reaches launchd, and returns its exit code, whether it would have
/// forwarded the call, and its log.
fn run_wrapper(dir: &Path, label: &str, args: &[&str]) -> (Option<i32>, bool, String) {
    use std::os::unix::fs::PermissionsExt as _;
    let log = dir.join("calls.log");
    let _ = std::fs::remove_file(&log);
    let script = launchctl_wrapper(label, &log);
    let forward = "exec /bin/launchctl \"$@\"\n";
    assert_eq!(script.matches(forward).count(), 1, "{script}");
    let script = script.replace(forward, "printf 'FORWARDED\\n'; exit 0\n");
    let wrapper = dir.join("launchctl");
    std::fs::write(&wrapper, script).expect("wrapper");
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).expect("mode");
    let output = Command::new(&wrapper)
        .args(args)
        .output()
        .expect("wrapper runs");
    let forwarded = String::from_utf8_lossy(&output.stdout).trim() == "FORWARDED";
    let calls = std::fs::read_to_string(&log).unwrap_or_default();
    (output.status.code(), forwarded, calls)
}

fn uid() -> String {
    let output = Command::new("id").arg("-u").output().expect("id -u");
    String::from_utf8(output.stdout)
        .expect("uid")
        .trim()
        .to_owned()
}

#[test]
fn the_launchctl_wrapper_forwards_only_print_of_the_scratch_service() {
    let dir = tempfile::tempdir().expect("tempdir");
    let label = "ai.commonmeasure.test-wrapper";
    let target = format!("gui/{}/{label}", uid());
    let (code, forwarded, calls) = run_wrapper(dir.path(), label, &["print", &target]);
    assert_eq!((code, forwarded), (Some(0), true), "{calls}");
    assert!(!calls.contains("REFUSED"), "{calls}");
}

#[test]
fn the_launchctl_wrapper_refuses_every_other_call_before_forwarding() {
    let dir = tempfile::tempdir().expect("tempdir");
    let label = "ai.commonmeasure.test-wrapper";
    let domain = format!("gui/{}", uid());
    // Named for the scratch service, registering another one.
    let plist = dir.path().join(format!("{label}.plist"));
    std::fs::write(
        &plist,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\"><dict>\
         <key>Label</key><string>ai.commonmeasure.test-wrapper-other</string>\
         <key>ProgramArguments</key><array><string>/usr/bin/true</string></array>\
         </dict></plist>\n",
    )
    .expect("plist");
    let plist = plist.display().to_string();
    let scratch = format!("{domain}/{label}");
    let prefixed = format!("{domain}/{label}0");
    let other_domain = format!("system/{label}");
    let cases: &[(&str, Vec<&str>)] = &[
        ("domain-only print", vec!["print", &domain]),
        ("domain-only bootout", vec!["bootout", &domain]),
        (
            "bootstrap of another Label",
            vec!["bootstrap", &domain, &plist],
        ),
        ("another domain", vec!["print", &other_domain]),
        (
            "bare-name remove",
            vec!["remove", "com.example.test-wrapper"],
        ),
        ("target-less list", vec!["list"]),
        (
            "label extending the scratch label",
            vec!["print", &prefixed],
        ),
        (
            "another operation on the scratch target",
            vec!["bootout", &scratch],
        ),
        ("an extra argument", vec!["print", &scratch, &scratch]),
    ];
    let mut passed = Vec::new();
    for (case, args) in cases {
        let (code, forwarded, calls) = run_wrapper(dir.path(), label, args);
        if forwarded || code != Some(97) || !calls.contains("REFUSED") {
            passed.push(format!("{case}: {args:?} exit {code:?}\n{calls}"));
        }
    }
    assert!(passed.is_empty(), "not refused:\n{}", passed.join("\n"));
}

#[cfg(debug_assertions)]
fn update(origin: &ServerHandle, binary: &Path, args: &[&str]) -> Output {
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    let label = format!(
        "ai.commonmeasure.test-installer-{}-{}",
        std::process::id(),
        CALLS.fetch_add(1, Ordering::Relaxed)
    );
    let home = binary.parent().unwrap().parent().unwrap();
    let tools = home.join("tools");
    std::fs::create_dir_all(&tools).expect("tools");
    let log = tools.join(format!("{label}.log"));
    let launchctl = tools.join("launchctl");
    std::fs::write(&launchctl, launchctl_wrapper(&label, &log)).expect("launchctl");
    std::fs::set_permissions(&launchctl, std::fs::Permissions::from_mode(0o755)).expect("mode");
    let path = std::env::join_paths(std::iter::once(tools.clone()).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .expect("PATH");

    let output = Command::new(binary)
        .arg("update")
        .args(args)
        .env("HOME", home)
        .env("PATH", path)
        .env("COMMONMEASURE_SERVICE_LABEL", &label)
        .env_remove("COMMONMEASURE_HOME")
        .env(
            "COMMONMEASURE_RELEASE_URL",
            format!("{}/{REPOSITORY}/releases", origin.url()),
        )
        .output()
        .expect("the binary runs");

    let calls = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !calls.contains("REFUSED"),
        "launchctl was asked about another target:\n{calls}"
    );
    for call in calls.lines() {
        assert!(
            call.contains(&label) && !call.contains("ai.commonmeasure.console"),
            "launchctl {call}"
        );
    }
    // An update that installed asked launchd about the service first.
    if cfg!(target_os = "macos")
        && String::from_utf8_lossy(&output.stdout).contains("checksum verified")
    {
        assert!(!calls.is_empty(), "launchd was asked under {label}");
    }
    output
}

#[cfg(debug_assertions)]
fn version_of(binary: &Path) -> String {
    let output = Command::new(binary)
        .arg("--version")
        .output()
        .expect("runs");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[test]
#[cfg(debug_assertions)]
fn update_replaces_this_binary_with_the_verified_latest_release() {
    let script = script_reporting("9.9.9");
    let sum = sha256_hex(&script);
    let origin = release("v9.9.9", script, &sum, true);
    let home = tempfile::tempdir().expect("tempdir");
    let binary = installed_copy(home.path());

    let output = update(&origin, &binary, &[]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(stdout.contains("checksum verified"), "{stdout}");
    assert!(
        !stdout.contains("Next:"),
        "an update is not a first install: {stdout}"
    );
    assert!(
        stdout.contains("sessions opened from now on run 9.9.9"),
        "{stdout}"
    );
    assert_eq!(version_of(&binary), "commonmeasure 9.9.9");
}

#[test]
#[cfg(debug_assertions)]
fn update_check_reports_both_versions_and_changes_nothing() {
    let script = script_reporting("9.9.9");
    let sum = sha256_hex(&script);
    let origin = release("v9.9.9", script, &sum, true);
    let home = tempfile::tempdir().expect("tempdir");
    let binary = installed_copy(home.path());

    let output = update(&origin, &binary, &["--check"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(
        stdout.contains(&format!("installed  {VERSION}")),
        "{stdout}"
    );
    assert!(stdout.contains("release    9.9.9"), "{stdout}");
    assert!(
        stdout.contains("Update with: commonmeasure update"),
        "{stdout}"
    );
    assert_eq!(version_of(&binary), format!("commonmeasure {VERSION}"));
}

#[test]
#[cfg(debug_assertions)]
fn update_does_nothing_when_this_is_the_latest_release() {
    let binary_bytes = this_binary();
    let sum = sha256_hex(&binary_bytes);
    let origin = release(&format!("v{VERSION}"), binary_bytes, &sum, true);
    let home = tempfile::tempdir().expect("tempdir");
    let binary = installed_copy(home.path());
    let before = std::fs::metadata(&binary).unwrap().modified().unwrap();

    let output = update(&origin, &binary, &[]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("the latest release is"), "{stdout}");
    assert_eq!(
        std::fs::metadata(&binary).unwrap().modified().unwrap(),
        before
    );
}

#[test]
#[cfg(debug_assertions)]
fn a_failed_update_leaves_this_binary_in_place() {
    let home = tempfile::tempdir().expect("tempdir");
    let binary = installed_copy(home.path());

    // The checksum list disagrees with the bytes served.
    let origin = release(
        "v9.9.9",
        script_reporting("9.9.9"),
        &sha256_hex(b"other"),
        true,
    );
    let output = update(&origin, &binary, &[]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("does not match its SHA-256"), "{stderr}");
    assert!(stderr.contains("is unchanged"), "{stderr}");
    assert_eq!(version_of(&binary), format!("commonmeasure {VERSION}"));

    // The binary served reports another version than its release.
    let script = script_reporting("9.9.8");
    let sum = sha256_hex(&script);
    let origin = release("v9.9.9", script, &sum, true);
    let output = update(&origin, &binary, &[]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("but the release is v9.9.9"), "{stderr}");
    assert_eq!(version_of(&binary), format!("commonmeasure {VERSION}"));
    let leftovers: Vec<_> = std::fs::read_dir(binary.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(leftovers, vec![std::ffi::OsString::from("commonmeasure")]);
}

#[test]
#[cfg(debug_assertions)]
fn update_prints_the_release_origin_before_it_changes_anything() {
    let script = script_reporting("9.9.9");
    let sum = sha256_hex(&script);
    let origin = release("v9.9.9", script, &sum, true);
    let home = tempfile::tempdir().expect("tempdir");
    let binary = installed_copy(home.path());

    let output = update(&origin, &binary, &["--check"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    // A debug build honours the override, so the tests' loopback origin is
    // the one named; a release build names the public repository.
    assert!(
        stdout.starts_with(&format!(
            "origin     {}/{REPOSITORY}/releases\n",
            origin.url()
        )),
        "{stdout}"
    );
}

/// A latest release whose tag is not `vX.Y.Z` is an error that downloads
/// nothing, and an explicit tag must have the same form.
#[test]
#[cfg(debug_assertions)]
fn update_refuses_a_latest_tag_it_cannot_compare() {
    for tag in [
        "vbanana",
        "v0.4.1-rc.1",
        "v99999999999999999999.0.0",
        "v01.4.1",
        "v1.2",
    ] {
        let script = script_reporting("9.9.9");
        let sum = sha256_hex(&script);
        let (origin, requests) = recorded_release(tag, script, &sum, true);
        let home = tempfile::tempdir().expect("tempdir");
        let binary = installed_copy(home.path());

        for args in [&[][..], &["--check"][..]] {
            let output = update(&origin, &binary, args);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(output.status.code(), Some(1), "{tag} {args:?}: {stderr}");
            assert!(
                stderr.contains(&format!("is tagged {tag}, which is not of the form vX.Y.Z")),
                "{tag}: {stderr}"
            );
        }
        let output = update(&origin, &binary, &["--tag", tag]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "--tag {tag}: {stderr}");
        assert!(
            stderr.contains("is not a release tag of the form vX.Y.Z"),
            "{stderr}"
        );
        assert_eq!(
            *requests.lock().unwrap(),
            vec![format!("/{REPOSITORY}/releases/latest"); 2],
            "{tag}: only latest was asked for"
        );
        assert_eq!(version_of(&binary), format!("commonmeasure {VERSION}"));
    }
}

#[test]
#[cfg(debug_assertions)]
fn update_does_not_downgrade_to_an_older_latest_but_installs_an_older_tag_by_name() {
    let script = script_reporting("0.0.1");
    let sum = sha256_hex(&script);
    let (origin, requests) = recorded_release("v0.0.1", script, &sum, true);
    let home = tempfile::tempdir().expect("tempdir");
    let binary = installed_copy(home.path());

    let output = update(&origin, &binary, &[]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("the latest release is 0.0.1"), "{stdout}");
    assert_eq!(
        *requests.lock().unwrap(),
        vec![format!("/{REPOSITORY}/releases/latest")]
    );
    assert_eq!(version_of(&binary), format!("commonmeasure {VERSION}"));

    let output = update(&origin, &binary, &["--tag", "v0.0.1"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert_eq!(version_of(&binary), "commonmeasure 0.0.1");
}

/// Another process running the binary, here an MCP server as a host starts
/// one, makes update refuse before anything is downloaded, naming the
/// process and its Edge home.
#[test]
#[cfg(debug_assertions)]
fn update_refuses_while_another_process_runs_this_binary() {
    let script = script_reporting("9.9.9");
    let sum = sha256_hex(&script);
    let (origin, requests) = recorded_release("v9.9.9", script, &sum, true);
    let home = tempfile::tempdir().expect("tempdir");
    let binary = installed_copy(home.path());
    let edge = home.path().join("edge");
    // The shape that once printed an environment entry: an empty argument
    // zero, and a credential-shaped variable first in the environment.
    let mut mcp = {
        use std::os::unix::process::CommandExt as _;
        Command::new(&binary)
            .arg0("")
            .args(["mcp", "--host", "claude-code"])
            .env_clear()
            .env("AAA_SENTINEL", "NOT_A_CREDENTIAL")
            .env("HOME", home.path())
            .env("COMMONMEASURE_HOME", &edge)
            .env("PATH", "/usr/bin:/bin")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("the MCP server starts")
    };
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(mcp.try_wait().unwrap().is_none(), "the MCP server runs");

    let output = update(&origin, &binary, &[]);
    let _ = mcp.kill();
    let _ = mcp.wait();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    // Pid, the executable as the kernel reports it, the subcommand and the
    // home; no other argument and no other environment entry.
    assert!(
        stderr.contains(&format!(
            "pid {}  {}  mcp  COMMONMEASURE_HOME={}",
            mcp.id(),
            binary.canonicalize().unwrap().display(),
            edge.display()
        )),
        "{stderr}"
    );
    for text in [&stdout, &stderr] {
        assert!(!text.contains("NOT_A_CREDENTIAL"), "{text}");
        assert!(!text.contains("AAA_SENTINEL"), "{text}");
        assert!(!text.contains("claude-code"), "{text}");
    }
    assert!(stderr.contains("quit the host app"), "{stderr}");
    assert!(
        stderr.contains("Nothing was stopped or installed"),
        "{stderr}"
    );
    assert!(
        !requests
            .lock()
            .unwrap()
            .iter()
            .any(|path| path.contains("/download/")),
        "nothing was downloaded"
    );
    assert_eq!(version_of(&binary), format!("commonmeasure {VERSION}"));

    // With it closed, the same update goes ahead.
    let output = update(&origin, &binary, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(version_of(&binary), "commonmeasure 9.9.9");
}
