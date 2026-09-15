//! Production CLI and MCP boundaries, real files and explicit isolated homes.
use commonmeasure_harness::{directory, policy::SessionPolicy};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

fn cli(home: &Path, cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(args)
        .current_dir(cwd)
        .env("COMMONMEASURE_HOME", home)
        .env("HOME", home)
        .env("CODEX_HOME", home.join("codex"))
        .output()
        .unwrap()
}
fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
fn mcp(home: &Path, cwd: &Path, requests: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["mcp", "--host", "codex", "--session", "directory-test"])
        .current_dir(cwd)
        .env("COMMONMEASURE_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    for request in requests {
        writeln!(child.stdin.as_mut().unwrap(), "{request}").unwrap();
    }
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
fn call(id: u64, args: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"context_enrol","arguments":args}})
}
fn answer(value: &Value) -> Value {
    serde_json::from_str(value["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn repeat_enrolment_boundaries_optout_and_policy_authority() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = temp.path().join("project");
    let child = root.join("src");
    let sibling = temp.path().join("project-copy");
    for path in [&home, &child, &sibling] {
        std::fs::create_dir_all(path).unwrap();
    }
    let original = r#"{"policy_mode":"strict","constraints":[{"kind":"denied_source_host","host":"blocked.example"}]}"#;
    std::fs::write(home.join("policy.json"), original).unwrap();
    let args = [
        "enrol",
        "--name",
        "Test project",
        "--reporting",
        "hub",
        "--include-history",
    ];
    success(cli(&home, &root, &args));
    let before = std::fs::read(home.join("directories.json")).unwrap();
    success(cli(&home, &root, &args));
    assert_eq!(
        before,
        std::fs::read(home.join("directories.json")).unwrap()
    );
    assert_eq!(
        original,
        std::fs::read_to_string(home.join("policy.json")).unwrap()
    );
    assert!(!home.join("edge-key.json").exists());
    for dir in [&root, &child] {
        assert!(
            SessionPolicy::load(&home, dir.to_str())
                .unwrap()
                .allows_telemetry_egress()
        );
    }
    assert!(
        !SessionPolicy::load(&home, sibling.to_str())
            .unwrap()
            .allows_telemetry_egress()
    );
    let p = SessionPolicy::load(&home, root.to_str()).unwrap();
    assert_eq!(p.describe()["mode"], "strict");
    success(cli(&home, &root, &["enrol", "--remove"]));
    assert!(
        !SessionPolicy::load(&home, child.to_str())
            .unwrap()
            .allows_telemetry_egress()
    );
    success(cli(&home, &root, &args));
    std::fs::write(
        home.join("policy.json"),
        json!({"scopes":[{"match":"project","engagement":"confidential"}]}).to_string(),
    )
    .unwrap();
    assert!(
        !SessionPolicy::load(&home, root.to_str())
            .unwrap()
            .allows_telemetry_egress(),
        "omitted egress bool is a veto too"
    );
    assert!(!cli(&home, Path::new("/"), &["enrol"]).status.success());
    assert!(
        !cli(&home, &root, &["enrol", "--name", "x"])
            .status
            .success()
    );
}

#[test]
fn mcp_requires_host_directory_agreement_and_updates_applied_status() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let home = temp.path().join("home");
    std::fs::create_dir(&root).unwrap();
    let result = mcp(
        &home,
        &root,
        &[
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","clientInfo":{"name":"directory-test","version":"1"}}}),
            json!({"jsonrpc":"2.0","id":2,"method":"prompts/list"}),
            call(
                3,
                json!({"directory":temp.path(),"action":"enrol","name":"Wrong","reporting":"local"}),
            ),
            call(
                4,
                json!({"directory":root,"action":"enrol","name":"Right","reporting":"local"}),
            ),
            call(5, json!({"directory":root,"action":"status"})),
        ],
    );
    assert!(result[0]["result"]["capabilities"].get("prompts").is_some());
    assert_eq!(result[2]["result"]["isError"], true);
    assert_eq!(answer(&result[3])["reporting"], "local_only");
    assert_eq!(
        answer(&result[4])["directory"],
        root.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(
        answer(&result[4])["first_evidence"]["state"],
        "no_witnessed_crossing"
    );
    let failed = mcp(
        &home,
        Path::new("/"),
        &[call(1, json!({"directory":root,"action":"status"}))],
    );
    assert_eq!(failed[0]["result"]["isError"], true);
}

#[test]
fn codex_skill_install_upgrade_remove_preserves_foreign_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    std::fs::create_dir(home.join("codex")).unwrap();
    let original = "# keep me\nmodel = \"example\"\n";
    std::fs::write(home.join("codex/config.toml"), original).unwrap();
    for _ in 0..2 {
        success(cli(home, home, &["install", "codex"]));
    }
    let skill = home.join(".agents/skills/commonmeasure-enrol/SKILL.md");
    assert!(skill.exists());
    let metadata = home.join(".agents/skills/commonmeasure-enrol/agents/openai.yaml");
    let installed = std::fs::read_to_string(&metadata).unwrap();
    std::fs::write(&metadata, "policy: {allow_implicit_invocation: true}\n").unwrap();
    assert!(!cli(home, home, &["install", "codex"]).status.success());
    assert_eq!(
        std::fs::read_to_string(&metadata).unwrap(),
        "policy: {allow_implicit_invocation: true}\n"
    );
    std::fs::write(&metadata, installed).unwrap();
    success(cli(home, home, &["uninstall", "codex"]));
    assert!(!skill.exists());
    assert_eq!(
        std::fs::read_to_string(home.join("codex/config.toml")).unwrap(),
        original
    );
    std::fs::write(&skill, "foreign skill").unwrap();
    assert!(!cli(home, home, &["install", "codex"]).status.success());
    assert_eq!(std::fs::read_to_string(&skill).unwrap(), "foreign skill");
    assert_eq!(
        std::fs::read_to_string(home.join("codex/config.toml")).unwrap(),
        original
    );
}

#[cfg(unix)]
#[test]
fn symlinks_and_git_worktrees_cannot_inherit_reporting_by_name() {
    use std::os::unix::fs::symlink;
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let root = t.path().join("main");
    let branch = t.path().join("branch");
    std::fs::create_dir_all(root.join(".git/worktrees/branch")).unwrap();
    std::fs::create_dir(&branch).unwrap();
    std::fs::write(
        branch.join(".git"),
        format!("gitdir: {}", root.join(".git/worktrees/branch").display()),
    )
    .unwrap();
    std::fs::write(root.join(".git/worktrees/branch/commondir"), "../..").unwrap();
    directory::Registry::enrol(&home, &root, "Main", true).unwrap();
    symlink(&root, t.path().join("alias")).unwrap();
    assert!(
        SessionPolicy::load(&home, t.path().join("alias").to_str())
            .unwrap()
            .allows_telemetry_egress()
    );
    assert!(
        !SessionPolicy::load(&home, branch.to_str())
            .unwrap()
            .allows_telemetry_egress()
    );
    std::fs::write(home.join("policy.json"),json!({"scopes":[{"match":"main","policy_mode":"strict","constraints":[{"kind":"denied_source_host","host":"blocked.example"}]}]}).to_string()).unwrap();
    directory::Registry::enrol(&home, &branch, "Branch", true).unwrap();
    let policy = SessionPolicy::load(&home, branch.to_str()).unwrap();
    assert!(!policy.allows_telemetry_egress());
    assert!(
        policy
            .describe()
            .to_string()
            .contains("linked Git worktree")
    );
}

#[test]
fn queued_evidence_is_rechecked_after_optout_and_can_be_reported_after_new_optin() {
    queued_consent_removal("optout", false);
}

#[test]
fn local_only_ancestor_vetoes_nested_git_repositories_and_submodules() {
    for submodule in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let root = temp.path().join("private");
        let nested = root.join("nested");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir(&nested).unwrap();
        if submodule {
            std::fs::create_dir_all(root.join(".git/modules/nested")).unwrap();
            std::fs::write(nested.join(".git"), "gitdir: ../.git/modules/nested\n").unwrap();
        } else {
            std::fs::create_dir(nested.join(".git")).unwrap();
        }
        let hub = [
            "enrol",
            "--name",
            "Project",
            "--reporting",
            "hub",
            "--include-history",
        ];
        success(cli(&home, &root, &hub));
        assert!(
            !SessionPolicy::load(&home, nested.to_str())
                .unwrap()
                .allows_telemetry_egress(),
            "positive reporting must not inherit across Git identities"
        );
        success(cli(
            &home,
            &root,
            &["enrol", "--name", "Private", "--reporting", "local"],
        ));
        success(cli(&home, &nested, &hub));
        let status: Value =
            serde_json::from_str(&success(cli(&home, &nested, &["enrol"]))).unwrap();
        assert_eq!(status["policy"]["allow_telemetry_egress"], false);
        assert_eq!(status["reporting"], "local_only");
        // A veto is valid only while the ancestor's own binding is valid.
        let registry = directory::Registry::read(&home).unwrap().unwrap();
        let mut invalid = registry.clone();
        invalid.projects[0].os_user = invalid.projects[0].os_user.wrapping_add(1);
        assert!(
            invalid
                .matching(nested.to_str().unwrap())
                .unwrap()
                .reporting
        );
        let mut invalid = registry.clone();
        invalid.projects[0].filesystem_id = "replaced-root".into();
        assert!(
            invalid
                .matching(nested.to_str().unwrap())
                .unwrap()
                .reporting
        );
        let mut invalid = registry.clone();
        invalid.projects[0].binding = "invalid-binding".into();
        assert!(
            invalid
                .matching(nested.to_str().unwrap())
                .unwrap()
                .reporting
        );
        let mut invalid = registry;
        invalid.projects[0].git_common = None;
        assert!(
            invalid
                .matching(nested.to_str().unwrap())
                .unwrap()
                .reporting
        );
    }
}

#[test]
fn missing_registry_withholds_queued_and_new_egress_even_with_a_legacy_scope() {
    for scope in [false, true] {
        queued_consent_removal("registry", scope);
    }
}

#[test]
fn queued_consent_provenance_survives_loss_of_registry_and_mode_marker() {
    for scope in [false, true] {
        queued_consent_removal("registry_and_marker", scope);
    }
}

#[test]
fn legacy_home_delivers_old_spool_entries_without_directory_provenance() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = temp.path().join("project");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::write(home.join("policy.json"), json!({"policy_mode":"observe", "scopes":[{"match":"project", "engagement":"legacy", "allow_telemetry_egress":true}]}).to_string()).unwrap();
    record_public(&home, &root, "legacy");
    let fail = Arc::new(AtomicBool::new(true));
    let accepted = Arc::new(AtomicUsize::new(0));
    let failing = fail.clone();
    let received = accepted.clone();
    let mut receiver = commonmeasure_http::Server::bind("127.0.0.1:0")
        .unwrap()
        .spawn(move |request| {
            if failing.load(Ordering::SeqCst) {
                commonmeasure_http::Response::text(503, "try later")
            } else {
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                assert!(
                    body.get("directory_selection").is_none(),
                    "local provenance must not enter the wire document"
                );
                let count = body["events"].as_array().unwrap().len();
                received.fetch_add(count, Ordering::SeqCst);
                commonmeasure_http::Response::json(
                    201,
                    &json!({"status":"ok", "events_created":count}).to_string(),
                )
            }
        })
        .unwrap();
    std::fs::write(
        home.join("relay.json"),
        json!({"receiver":receiver.url()}).to_string(),
    )
    .unwrap();
    assert!(!cli(&home, &root, &["relay"]).status.success());
    let spool = home.join("relay/spool/outbound.ndjson");
    let mut entry: Value =
        serde_json::from_str(std::fs::read_to_string(&spool).unwrap().trim()).unwrap();
    assert_eq!(entry["directory_selection"], false);
    entry.as_object_mut().unwrap().remove("directory_selection");
    std::fs::write(spool, format!("{entry}\n")).unwrap();
    fail.store(false, Ordering::SeqCst);
    success(cli(&home, &root, &["relay"]));
    assert_eq!(accepted.load(Ordering::SeqCst), 2);
    success(cli(&home, &root, &["relay"]));
    assert_eq!(accepted.load(Ordering::SeqCst), 2);
    assert!(!home.join("directory-selection.json").exists());
    assert!(directory::Registry::read(&home).unwrap().is_none());
    receiver.stop();
}

fn record_public(home: &Path, root: &Path, session: &str) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["hook", "post-tool-use"])
        .env("COMMONMEASURE_HOME", home)
        .env("HOME", home)
        .env("CODEX_HOME", home.join("codex"))
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.as_mut().unwrap(), "{}", json!({"session_id":session,"cwd":root,"tool_name":"WebFetch","tool_input":{"url":"https://public.example/article"},"tool_response":{"result":"Public fixture text, witnessed through the production hook."}})).unwrap();
    drop(child.stdin.take());
    assert!(child.wait().unwrap().success());
}

// A loopback fault-injection receiver proves withholding/retry behaviour; it
// does not establish a live Hub or external supplier integration.
fn queued_consent_removal(removal: &str, legacy_scope: bool) {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let root = t.path().join("project");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&home).unwrap();
    let scopes = if legacy_scope {
        json!([{"match":"project", "engagement":"legacy", "allow_telemetry_egress":true}])
    } else {
        json!([])
    };
    std::fs::write(
        home.join("policy.json"),
        json!({"policy_mode":"observe", "scopes":scopes}).to_string(),
    )
    .unwrap();
    success(cli(
        &home,
        &root,
        &[
            "enrol",
            "--name",
            "Test",
            "--reporting",
            "hub",
            "--include-history",
        ],
    ));
    record_public(&home, &root, "queued");
    let fail = Arc::new(AtomicBool::new(true));
    let attempts = Arc::new(AtomicUsize::new(0));
    let failing = fail.clone();
    let sent = attempts.clone();
    let mut receiver=commonmeasure_http::Server::bind("127.0.0.1:0").unwrap().spawn(move |request| {
        sent.fetch_add(1,Ordering::SeqCst);
        if failing.load(Ordering::SeqCst) { commonmeasure_http::Response::text(503,"try later") } else {
            let body:Value=serde_json::from_slice(&request.body).unwrap();
            commonmeasure_http::Response::json(201,&json!({"status":"ok","events_created":body["events"].as_array().unwrap().len()}).to_string())
        }
    }).unwrap();
    std::fs::write(
        home.join("relay.json"),
        json!({"receiver":receiver.url()}).to_string(),
    )
    .unwrap();
    assert!(!cli(&home, &root, &["relay"]).status.success());
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    let evidence = std::fs::read(home.join("sessions/queued.ndjson")).unwrap();
    let queue = std::fs::read_to_string(home.join("relay/spool/outbound.ndjson")).unwrap();
    let entry: Value = serde_json::from_str(queue.lines().next().unwrap()).unwrap();
    assert_eq!(entry["directory_selection"], true);
    match removal {
        "optout" => {
            success(cli(&home, &root, &["enrol", "--remove"]));
        }
        "registry" | "registry_and_marker" => {
            std::fs::remove_file(home.join("directories.json")).unwrap();
            assert!(home.join("directory-selection.json").exists());
        }
        _ => unreachable!(),
    }
    assert!(
        !SessionPolicy::load(&home, root.to_str())
            .unwrap()
            .allows_telemetry_egress()
    );
    // Fresh evidence and queued evidence must both stay local when the
    // registry is missing, even if the source policy contains a legacy grant.
    if removal != "registry_and_marker" {
        record_public(&home, &root, "after-removal");
    }
    let preview = success(cli(&home, &root, &["relay", "--dry-run"]));
    assert!(preview.contains("would deliver 0 events"), "{preview}");
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    if removal == "registry_and_marker" {
        // Exercise durable queued provenance independently of the mode marker.
        std::fs::remove_file(home.join("directory-selection.json")).unwrap();
    }
    fail.store(false, Ordering::SeqCst);
    if removal == "registry_and_marker" {
        for _ in 0..2 {
            let retry = cli(&home, &root, &["relay"]);
            assert!(!retry.status.success());
            assert!(
                String::from_utf8_lossy(&retry.stderr).contains("requires local consent state")
            );
        }
    } else {
        success(cli(&home, &root, &["relay"]));
    }
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "queued consent was removed before retry"
    );
    assert_eq!(
        evidence,
        std::fs::read(home.join("sessions/queued.ndjson")).unwrap()
    );
    success(cli(
        &home,
        &root,
        &[
            "enrol",
            "--name",
            "Test",
            "--reporting",
            "hub",
            "--include-history",
        ],
    ));
    success(cli(&home, &root, &["relay"]));
    let expected_attempts = if removal == "registry_and_marker" {
        2
    } else {
        3
    };
    assert_eq!(attempts.load(Ordering::SeqCst), expected_attempts);
    success(cli(&home, &root, &["relay"]));
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        expected_attempts,
        "delivery is idempotent"
    );
    receiver.stop();
}

#[test]
fn nested_linked_worktrees_require_separate_reporting_consent() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = temp.path().join("project");
    std::fs::create_dir(&root).unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(&root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "--quiet"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--allow-empty",
        "-m",
        "fixture",
    ]);
    git(&["worktree", "add", "-b", "nested", "nested-worktree"]);
    let nested = root.join("nested-worktree");
    let args = [
        "enrol",
        "--name",
        "Project",
        "--reporting",
        "hub",
        "--include-history",
    ];
    success(cli(&home, &root, &args));
    assert!(
        !SessionPolicy::load(&home, nested.to_str())
            .unwrap()
            .allows_telemetry_egress()
    );
    let status: Value = serde_json::from_str(&success(cli(&home, &nested, &["enrol"]))).unwrap();
    assert!(status["project"].is_null());
    success(cli(&home, &nested, &args));
    assert!(
        SessionPolicy::load(&home, nested.to_str())
            .unwrap()
            .allows_telemetry_egress()
    );
    success(cli(
        &home,
        &root,
        &["enrol", "--name", "Project", "--reporting", "local"],
    ));
    assert!(
        !SessionPolicy::load(&home, nested.to_str())
            .unwrap()
            .allows_telemetry_egress()
    );
}
