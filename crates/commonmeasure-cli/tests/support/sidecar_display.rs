//! Sidecar protocol and policy acceptance through the real stdio edge.
//! The fixed-probability example and fault scripts establish the boundary,
//! not the accuracy of an inference model.
#![cfg(unix)]

use super::*;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const PARAPHRASE: &str = "The bridge started operating on Tuesday.";
const CONTRADICTION: &str = "The bridge was never opened.";
const HEADER: &str = "# /// script\n# requires-python = \">=3.11\"\n# dependencies = []\n# ///\n";
const HELLO: &str = "print(json.dumps({'hello': {'model': 'fixture/1', 'family': 'fixture', 'questions': ['supported', 'contradicts']}}), flush=True)\n";

fn labelled_origin() -> ServerHandle {
    origin(resigned_body(|r| {
        r["sidecar"]["signed"]["display_mode"] = json!("labelled")
    }))
}

fn items() -> Value {
    json!([
        {"kind": "from_source", "text": PARAPHRASE, "passages": ["p0"]},
        {"kind": "from_source", "text": CONTRADICTION, "passages": ["p0"]},
    ])
}

fn manifest(dir: &Path, script: &str, timeout: f64, network: Value) -> PathBuf {
    let script = format!("{HEADER}{script}");
    std::fs::write(dir.join("checker.py"), &script).unwrap();
    let value = json!({
        "protocol": "commonmeasure-sidecar/1", "name": "fixture", "version": "1",
        "stage": "verify", "capability": "display-integrity-model",
        "artefact": {"path": "checker.py", "sha256": sha256_digest(script.as_bytes()).trim_start_matches("sha256:")},
        "runtime": {"command": ["uv", "run", "--script"]},
        "model": true, "network": network, "determinism": "deterministic",
        "timeout_seconds": timeout, "questions": ["supported", "contradicts"],
    });
    let path = dir.join("sidecar.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    path
}

fn configured(home: &Path, path: &Path, extra: &[(&str, &Path)]) -> Edge {
    write_policy(
        home,
        r#"{"policy_mode":"observe","allow_private_hosts":true}"#,
    );
    configured_under_policy(home, path, extra)
}

fn configured_under_policy(home: &Path, path: &Path, extra: &[(&str, &Path)]) -> Edge {
    let mut env = vec![
        ("COMMONMEASURE_CHECKER", path),
        ("COMMONMEASURE_CHECKER_THRESHOLD", Path::new("0.9")),
    ];
    env.extend_from_slice(extra);
    Edge::start_under_policy(home, &[], &env)
}

fn invoke(edge: &mut Edge, site: &ServerHandle) -> Value {
    let fetched = edge.fetch(&format!("{}/article", site.url()));
    let answer = edge.show(fetched["content_hash"].as_str().unwrap(), items());
    assert_eq!(answer["result"]["isError"], false, "{answer}");
    payload(&answer)
}

fn invocation(home: &Path) -> Value {
    records(home)
        .into_iter()
        .rev()
        .find(|r| r["payload"]["processor"]["name"] == "integrity-checker")
        .unwrap()["payload"]
        .clone()
}

fn check(path: &Path, home: &Path) -> (bool, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_commonmeasure"))
        .args(["sidecar", "check"])
        .arg(path)
        .env("COMMONMEASURE_HOME", home)
        .output()
        .unwrap();
    let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{output:?}"));
    (output.status.success(), value)
}

#[test]
fn sidecar_example_confirms_and_withholds_with_a_completed_record() {
    let home = tempfile::tempdir().unwrap();
    let site = labelled_origin();
    let path = repo_root().join("sidecars/example/sidecar.json");
    let mut edge = configured(home.path(), &path, &[]);
    let before = payload(&edge.ask(call("context_status", json!({}))));
    assert_eq!(before["checker"]["reachable"], Value::Null, "{before}");
    let display = invoke(&mut edge, &site);
    assert_eq!(
        outcomes(&display),
        ["shown_confirmed", "withheld_contradiction"]
    );
    assert_eq!(vector(&display)[4], 1);
    assert_eq!(vector(&display)[7], 1);
    let record = invocation(home.path());
    let model = &record["detail"]["model"];
    assert_eq!(model["outcome"], "completed", "{model}");
    assert_eq!(model["hello"]["model"], "fixed-probabilities/1");
    assert_eq!(model["bye"]["requests"], 2);
    assert_eq!(model["threshold"], 0.9);
    assert_eq!(model["temperature"], 1.0);
    assert_eq!(model["calibration"]["state"], "uncalibrated");
    assert_eq!(record["processor"]["implementation"]["kind"], "sidecar");
    assert_eq!(record["processor"]["permissions"]["network"], false);
    assert_eq!(
        record["detail"]["items"][0]["answers"][4]["reason"],
        "sidecar_probability_reported"
    );
    let after = payload(&edge.ask(call("context_status", json!({}))));
    assert_eq!(after["checker"]["reachable"], true, "{after}");
    assert_eq!(after["checker"]["last_outcome"], "completed");
    assert_eq!(after["checker"]["confinement"], "declared");
    assert_eq!(after["checker"]["hello"]["family"], "example");
    let (ok, report) = check(&path, home.path());
    assert!(ok, "{report}");
    assert_eq!(report["lifecycle"]["bye"]["requests"], 1);
}

#[test]
fn sidecar_echoes_never_enter_the_source_record_or_server_log() {
    let home = tempfile::tempdir().unwrap();
    let site = labelled_origin();
    let path = manifest(
        home.path(),
        &format!(
            "import json, sys\n{HELLO}\nfor line in sys.stdin:\n c = json.loads(line)\n print(c['summary'], c['passage'], file=sys.stderr, flush=True)\n print(json.dumps({{'id': c['id'], 'p_supported': .8, 'p_contradicts': .1, 'latency_ms': 1, 'detail': {{'p_raw': c['summary'], 'reason': c['passage'], c['summary']: c['passage'], 'bounded_letters': [c['summary']], 'decisions': [{{'probabilities': {{c['summary']: .9}}}}]}}}}), flush=True)\nprint(json.dumps({{'bye': {{'requests': 2, 'echo': c['summary']}}}}), flush=True)\n"
        ),
        10.0,
        json!(false),
    );
    let mut edge = configured(
        home.path(),
        &path,
        &[("CM_SIDECAR_SECRET_SENTINEL", Path::new("not-for-sidecars"))],
    );
    invoke(&mut edge, &site);
    let record = invocation(home.path()).to_string();
    for text in [PARAPHRASE, CONTRADICTION, recorded_text().as_str()] {
        assert!(!record.contains(text), "{record}");
        let log =
            std::fs::read_to_string(home.path().join("sessions/test-session.ndjson")).unwrap();
        assert!(!log.contains(text));
        assert!(
            !std::fs::read_to_string(home.path().join("stderr.log"))
                .unwrap()
                .contains(text)
        );
    }
    assert!(record.contains("sidecar_output_dropped"));
    let (ok, report) = check(&path, home.path());
    assert!(!ok && report.to_string().contains("sidecar_output_dropped"));
}

fn bounded_output_record(hello: Value, detail: Value) -> Value {
    let home = tempfile::tempdir().unwrap();
    let site = labelled_origin();
    let path = manifest(
        home.path(),
        &format!(
            r#"import json, sys
print(json.dumps({{'hello': json.loads({hello:?})}}), flush=True)
for line in sys.stdin:
 c = json.loads(line)
 print(json.dumps({{'id': c['id'], 'p_supported': .99, 'p_contradicts': .01, 'latency_ms': 0, 'detail': json.loads({detail:?})}}), flush=True)
print(json.dumps({{'bye': {{'requests': 2}}}}), flush=True)
"#,
            hello = hello.to_string(),
            detail = detail.to_string(),
        ),
        10.0,
        json!(false),
    );
    let mut edge = configured(home.path(), &path, &[]);
    assert_eq!(
        outcomes(&invoke(&mut edge, &site)),
        ["shown_confirmed", "shown_confirmed"]
    );
    let record = invocation(home.path());
    assert_eq!(record["detail"]["model"]["outcome"], "completed");
    assert!(record.to_string().contains("sidecar_output_dropped"));
    record
}

fn bounded_hello() -> Value {
    json!({"model": "fixture/1", "family": "fixture", "device": "cpu",
        "questions": ["supported", "contradicts"]})
}

fn assert_hello_not_applicable(hello: Value, absent: &[&str]) {
    let home = tempfile::tempdir().unwrap();
    let site = labelled_origin();
    let path = manifest(
        home.path(),
        &format!(
            r#"import json, sys
print(json.dumps({{'hello': json.loads({hello:?})}}), flush=True)
for line in sys.stdin:
 c = json.loads(line)
 print(json.dumps({{'id': c['id'], 'p_supported': .99, 'p_contradicts': .01, 'latency_ms': 0}}), flush=True)
print(json.dumps({{'bye': {{'requests': 2}}}}), flush=True)
"#,
            hello = hello.to_string(),
        ),
        10.0,
        json!(false),
    );
    let mut edge = configured(home.path(), &path, &[]);
    let display = invoke(&mut edge, &site);
    assert_eq!(
        outcomes(&display),
        ["shown_unconfirmed", "shown_unconfirmed"]
    );
    assert_eq!(vector(&display)[7], 0);
    let record = invocation(home.path());
    assert_eq!(record["detail"]["model"]["outcome"], "failed");
    for item in record["detail"]["items"].as_array().unwrap() {
        for answer in item["answers"].as_array().unwrap().iter().skip(4) {
            assert_eq!(answer["label"], "not_applicable");
            assert_eq!(answer["probability"], Value::Null);
            assert_eq!(answer["reason"], "sidecar_hello_invalid");
        }
    }
    let log = std::fs::read_to_string(home.path().join("sessions/test-session.ndjson")).unwrap();
    let stderr = std::fs::read_to_string(home.path().join("stderr.log")).unwrap();
    for text in absent {
        assert!(!log.contains(text));
        assert!(!stderr.contains(text));
    }
    assert!(record.to_string().contains("sidecar_output_dropped"));
    assert!(record.to_string().contains("sidecar_hello_invalid"));
}

#[test]
fn sidecar_overlong_hello_model_fails_without_recording_the_value() {
    let mut hello = bounded_hello();
    let model = "m".repeat(65);
    hello["model"] = json!(model);
    assert_hello_not_applicable(hello, &[&model]);
}

#[test]
fn sidecar_missing_hello_model_is_not_applicable() {
    let mut hello = bounded_hello();
    hello.as_object_mut().unwrap().remove("model");
    assert_hello_not_applicable(hello, &[]);
}

#[test]
fn sidecar_required_hello_identities_must_pass_the_filter() {
    for key in ["model", "family"] {
        for value in [
            None,
            Some(json!(null)),
            Some(json!(42)),
            Some(json!("")),
            Some(json!("f".repeat(65))),
            Some(json!("bad identity")),
            Some(json!("/path")),
            Some(json!("~home")),
            Some(json!(".hidden")),
        ] {
            let mut hello = bounded_hello();
            if let Some(value) = value {
                hello[key] = value;
            } else {
                hello.as_object_mut().unwrap().remove(key);
            }
            assert_hello_not_applicable(hello, &[]);
        }
    }
}

#[test]
fn sidecar_hello_device_path_is_omitted() {
    let mut hello = bounded_hello();
    hello["device"] = json!("/home/fixture/device");
    let record = bounded_output_record(hello, json!({}));
    assert!(record["detail"]["model"]["hello"].get("device").is_none());
    assert!(!record.to_string().contains("/home/fixture/device"));
}

#[test]
fn sidecar_chunk_probabilities_longer_than_chunks_are_omitted() {
    let record = bounded_output_record(
        bounded_hello(),
        json!({"chunks": 1, "p_per_chunk": [0.8, 0.9]}),
    );
    for item in record["detail"]["items"].as_array().unwrap() {
        let reported = &item["answers"][4]["detail"]["reported"];
        assert_eq!(reported["chunks"], 1);
        assert!(reported.get("p_per_chunk").is_none());
    }
}

#[test]
fn sidecar_nested_decisions_are_omitted() {
    let record = bounded_output_record(
        bounded_hello(),
        json!({"decisions": [{"p_raw": 0.9, "decisions": [{"p_raw": 0.8}]}]}),
    );
    for item in record["detail"]["items"].as_array().unwrap() {
        let reported = &item["answers"][4]["detail"]["reported"];
        assert_eq!(reported["decisions"], json!([{"p_raw": 0.9}]));
        assert!(reported["decisions"][0].get("decisions").is_none());
    }
}

#[test]
fn sidecar_faults_are_not_applicable_and_the_check_verb_names_them() {
    let site = labelled_origin();
    let fixtures = [
        (
            "print('not-json', flush=True)",
            "sidecar_protocol_malformed",
        ),
        (
            "print(json.dumps({'id': c['id'], 'p_supported': .99, 'latency_ms': 0}), flush=True)",
            "sidecar_answer_malformed",
        ),
        (
            "print(json.dumps({'id': c['id'], 'p_supported': 2, 'p_contradicts': .1, 'latency_ms': 0}), flush=True)",
            "sidecar_answer_malformed",
        ),
        (
            "print(json.dumps({'id': c['id'], 'p_supported': .99, 'p_contradicts': .99, 'latency_ms': 0, 'error': c['summary']}), flush=True)",
            "sidecar_item_error",
        ),
        (
            "print(json.dumps({'id': 'wrong', 'p_supported': .99, 'p_contradicts': .99, 'latency_ms': 0}), flush=True)",
            "sidecar_answer_order_invalid",
        ),
    ];
    for (answer, fault) in fixtures {
        let home = tempfile::tempdir().unwrap();
        let script = format!(
            "import json, sys\n{HELLO}\nfor line in sys.stdin:\n c = json.loads(line)\n {answer}\nprint(json.dumps({{'bye': {{'requests': 2}}}}), flush=True)\n"
        );
        let path = manifest(home.path(), &script, 10.0, json!(false));
        let mut edge = configured(home.path(), &path, &[]);
        let display = invoke(&mut edge, &site);
        assert_eq!(
            outcomes(&display),
            ["shown_unconfirmed", "shown_unconfirmed"],
            "{fault}: {display}"
        );
        assert_eq!(vector(&display)[7], 0);
        let record = invocation(home.path());
        assert!(record.to_string().contains(fault), "{record}");
        for item in record["detail"]["items"].as_array().unwrap() {
            for a in item["answers"].as_array().unwrap().iter().skip(4) {
                assert_eq!(a["label"], "not_applicable");
                assert_eq!(a["probability"], Value::Null);
            }
        }
        let (ok, report) = check(&path, home.path());
        assert!(!ok && report.to_string().contains(fault), "{report}");
    }
    // Even apparently valid replies and bye cannot confirm after exit 3.
    let home = tempfile::tempdir().unwrap();
    let path = manifest(
        home.path(),
        &format!(
            "import json, sys\n{HELLO}\nfor line in sys.stdin:\n c=json.loads(line)\n print(json.dumps({{'id': c['id'], 'p_supported': .99, 'p_contradicts': .99, 'latency_ms': 0}}), flush=True)\nprint(json.dumps({{'bye': {{'requests': 2}}}}), flush=True)\nsys.exit(3)\n"
        ),
        10.0,
        json!(false),
    );
    let mut edge = configured(home.path(), &path, &[]);
    assert_eq!(
        outcomes(&invoke(&mut edge, &site)),
        ["shown_unconfirmed", "shown_unconfirmed"]
    );
    assert!(
        invocation(home.path())
            .to_string()
            .contains("sidecar_exit_failed")
    );
    let (ok, report) = check(&path, home.path());
    assert!(
        !ok && report.to_string().contains("sidecar_exit_failed"),
        "{report}"
    );
}

#[test]
fn sidecar_bad_manifests_start_nothing_even_for_status() {
    let site = labelled_origin();
    for (field, value, fault) in [
        ("digest", json!("0".repeat(64)), "artefact_digest_mismatch"),
        ("protocol", json!("other/1"), "sidecar_protocol_unsupported"),
        (
            "runtime",
            json!({"command": ["python3"]}),
            "sidecar_runtime_unsupported",
        ),
    ] {
        let home = tempfile::tempdir().unwrap();
        let marker = home.path().join("started");
        let path = manifest(
            home.path(),
            &format!("open({:?}, 'w').write('started')\n", marker),
            10.0,
            json!(false),
        );
        let mut m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        if field == "digest" {
            m["artefact"]["sha256"] = value;
        } else {
            m[field] = value;
        }
        std::fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();
        let mut edge = configured(home.path(), &path, &[]);
        let status = payload(&edge.ask(call("context_status", json!({}))));
        assert_eq!(status["checker"]["gap"], fault);
        assert!(!marker.exists());
        invoke(&mut edge, &site);
        assert!(!marker.exists());
        let (ok, report) = check(&path, home.path());
        assert!(!ok && report.to_string().contains(fault), "{report}");
        assert!(!marker.exists());
    }
}

#[cfg(unix)]
#[test]
fn sidecar_hello_and_batch_deadlines_kill_the_whole_process_group() {
    let site = labelled_origin();
    for hello in [false, true] {
        for strict in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let pgid = home.path().join("group");
            let script = format!(
                "import json, sys, os, time\nopen({:?}, 'w').write(str(os.getpgrp()))\nif os.fork() == 0:\n time.sleep(60)\n os._exit(0)\n{}\ntime.sleep(60)\n",
                pgid,
                if hello { HELLO } else { "" }
            );
            let path = manifest(home.path(), &script, 1.0, json!(false));
            let mut edge = configured(home.path(), &path, &[]);
            let strict_site = origin(recorded_body());
            let display = invoke(&mut edge, if strict { &strict_site } else { &site });
            assert_eq!(vector(&display)[7], 0);
            assert_eq!(
                outcomes(&display),
                if strict {
                    ["withheld_unconfirmed", "withheld_unconfirmed"]
                } else {
                    ["shown_unconfirmed", "shown_unconfirmed"]
                }
            );
            let fault = if hello {
                "sidecar_batch_timeout"
            } else {
                "sidecar_hello_timeout"
            };
            assert!(invocation(home.path()).to_string().contains(fault));
            let group: i32 = std::fs::read_to_string(pgid).unwrap().parse().unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                // SAFETY: signal 0 only observes the process group made by
                // this fixture. It sends no signal to other processes.
                if unsafe { libc::kill(-group, 0) } == -1 {
                    assert_eq!(
                        std::io::Error::last_os_error().raw_os_error(),
                        Some(libc::ESRCH)
                    );
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "process group {group} survived {fault}"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            let (ok, report) = check(&path, home.path());
            assert!(!ok && report.to_string().contains(fault), "{report}");
        }
    }
}

#[test]
fn sidecar_summary_thresholds_and_shortening_only_timeout_are_recorded() {
    let site = labelled_origin();
    let home = tempfile::tempdir().unwrap();
    let path = repo_root().join("sidecars/example/sidecar.json");
    let summary = home.path().join("summary.json");
    std::fs::write(&summary, r#"{"checkers":{"fixed-probabilities":{"per_set":{"all":{"temperature":2.0,"by_target":[{"target":0.05,"threshold":0.8}]}}}}}"#).unwrap();
    let mut edge = configured(
        home.path(),
        &path,
        &[
            ("COMMONMEASURE_CHECKER_SUMMARY", &summary),
            ("COMMONMEASURE_CHECKER_TIMEOUT_SECONDS", Path::new("99")),
        ],
    );
    invoke(&mut edge, &site);
    let r = invocation(home.path());
    assert_eq!(r["detail"]["model"]["threshold"], 0.8);
    assert_eq!(r["detail"]["model"]["temperature"], 2.0);
    assert_eq!(r["detail"]["model"]["calibration"]["state"], "calibrated");
    assert_eq!(r["processor"]["limits"]["hello_timeout_seconds"], 10.0);
    assert_eq!(r["processor"]["limits"]["batch_timeout_seconds"], 10.0);
    std::fs::write(&summary, r#"{"checkers":{}}"#).unwrap();
    invoke(&mut edge, &site);
    assert!(
        invocation(home.path())
            .to_string()
            .contains("sidecar_summary_invalid")
    );
}

#[test]
fn sidecar_network_requires_its_exact_policy_term_and_scrubs_the_environment() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    let mock = Server::bind("127.0.0.1:0").unwrap().spawn(move |request| {
        observed.fetch_add(1, Ordering::SeqCst);
        assert_eq!(request.headers.get("Authorization"), Some("Bearer test-key-not-a-secret"));
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["questions"]["grounding"]["type"], "choice");
        let summary = body["state"]["sentence"].as_str().unwrap();
        if summary == CONTRADICTION {
            Response::text(200, r#"{"model":"fixture/1","answers":{"grounding":{"type":"choice","probabilities":{"supports":0.01,"absent":0.01,"contradicts":0.98}}},"usage":{"input_tokens":23}}"#)
        } else {
            Response::text(200, r#"{"model":"fixture/1","answers":{"grounding":{"type":"choice","probabilities":{"supports":0.98,"absent":0.01,"contradicts":0.01}}},"usage":{"input_tokens":23}}"#)
        }
    }).unwrap();
    let site = labelled_origin();
    let home = tempfile::tempdir().unwrap();
    // The script reaches the loopback System One shape using Python's real
    // HTTP transport. Only its declared credential is inherited.
    let script = format!(
        r#"import json, sys, os, urllib.request
assert 'CM_SIDECAR_SECRET_SENTINEL' not in os.environ
assert 'COMMONMEASURE_HOME' not in os.environ
{HELLO}
requests = 0
for line in sys.stdin:
 c = json.loads(line)
 body = {{'model': 'fixture/1', 'state': {{'passage': c['passage'], 'sentence': c['summary']}}, 'questions': {{'grounding': {{'type': 'choice'}}}}}}
 req = urllib.request.Request({endpoint:?}, data=json.dumps(body).encode(), headers={{'Authorization': 'Bearer ' + os.environ['FIXTURE_KEY'], 'Content-Type': 'application/json'}})
 with urllib.request.urlopen(req, timeout=2) as response:
  r = json.load(response)
 p = r['answers']['grounding']['probabilities']
 print(json.dumps({{'id': c['id'], 'p_supported': p['supports'], 'p_contradicts': p['contradicts'], 'latency_ms': 1}}), flush=True)
 requests += 1
print(json.dumps({{'bye': {{'requests': requests, 'input_tokens': requests * 23}}}}), flush=True)
"#,
        endpoint = format!("{}/v1/systemone", mock.url())
    );
    let path = manifest(
        home.path(),
        &script,
        10.0,
        json!({"provider": "loopback", "origins": [mock.url()], "credential_env": "FIXTURE_KEY"}),
    );
    let extra = [
        ("FIXTURE_KEY", Path::new("test-key-not-a-secret")),
        ("CM_SIDECAR_SECRET_SENTINEL", Path::new("must-not-cross")),
    ];
    let mut edge = configured(home.path(), &path, &extra);
    invoke(&mut edge, &site);
    assert_eq!(requests.load(Ordering::SeqCst), 0);
    assert!(
        invocation(home.path())
            .to_string()
            .contains("processor_egress_refused")
    );
    drop(edge);
    for mode in ["observe", "prefer", "strict"] {
        write_policy(home.path(), &json!({"policy_mode": mode, "allow_private_hosts": true,
            "constraints": [{"kind": "processor_egress", "processor": "other", "provider": "loopback"}]}).to_string());
        let mut edge = configured_under_policy(home.path(), &path, &extra);
        invoke(&mut edge, &site);
        assert_eq!(requests.load(Ordering::SeqCst), 0);
    }
    let term = json!({"kind": "processor_egress", "processor": "fixture", "provider": "loopback"});
    write_policy(
        home.path(),
        &json!({"policy_mode": "strict", "allow_private_hosts": true,
        "constraints": [term]})
        .to_string(),
    );
    let mut edge = configured_under_policy(home.path(), &path, &extra);
    let display = invoke(&mut edge, &site);
    assert_eq!(
        outcomes(&display),
        ["shown_confirmed", "withheld_contradiction"],
        "{display}"
    );
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    let record = invocation(home.path());
    let egress = &record["detail"]["model"]["egress"];
    assert_eq!(egress["admitting_term"], term);
    assert_eq!(egress["origins"], json!([mock.url()]));
    assert_eq!(egress["confinement"], "declared");
    assert_eq!(egress["usage"]["input_tokens"], 46);
    assert_eq!(egress["threshold"], 0.9);
    assert!(
        egress["requests"][0]["request_sha256"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    assert_eq!(egress["model"], "fixture/1");
    let status = payload(&edge.ask(call("context_status", json!({}))));
    assert_eq!(status["checker"]["credential"]["state"], "set");
    assert_eq!(requests.load(Ordering::SeqCst), 2, "status makes no call");
    let log = std::fs::read_to_string(home.path().join("sessions/test-session.ndjson")).unwrap();
    assert!(!log.contains("test-key-not-a-secret"));
    assert!(!log.contains("must-not-cross"));
    let log_path = home.path().join("sessions/test-session.ndjson");
    let (ok, report, error) = commonmeasure(&["inspect".as_ref(), log_path.as_os_str()]);
    assert!(ok, "{error}");
    for expected in [
        "model manifest",
        "model hello",
        "model bye",
        "model egress",
        "declared",
        "loopback",
        "sidecar-model/1",
    ] {
        assert!(report.contains(expected), "missing {expected}: {report}");
    }
    for absent in [
        PARAPHRASE,
        CONTRADICTION,
        "test-key-not-a-secret",
        "must-not-cross",
    ] {
        assert!(!report.contains(absent), "{report}");
    }
}

#[test]
fn sidecar_loopback_provider_faults_never_confirm_or_leak_provider_text() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    for fault in ["missing", "rate_limit", "timeout"] {
        let seen = Arc::new(AtomicUsize::new(0));
        let calls = seen.clone();
        let mock = Server::bind("127.0.0.1:0")
            .unwrap()
            .spawn(move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                match fault {
                    "missing" => {
                        Response::text(200, r#"{"answers":{"grounding":{"type":"choice"}}}"#)
                    }
                    "rate_limit" => {
                        Response::text(429, "licensed-provider-text-must-not-be-recorded")
                    }
                    _ => {
                        std::thread::sleep(Duration::from_millis(150));
                        Response::text(200, "late-provider-text-must-not-be-recorded")
                    }
                }
            })
            .unwrap();
        let home = tempfile::tempdir().unwrap();
        let script = format!(
            r#"import json, sys, os, urllib.request
{HELLO}
for line in sys.stdin:
 c=json.loads(line)
 try:
  req=urllib.request.Request({endpoint:?}, data=json.dumps({{'state': {{'passage': c['passage'], 'sentence': c['summary']}}, 'questions': {{'grounding': {{'type': 'choice'}}}}}}).encode())
  with urllib.request.urlopen(req, timeout=.05) as response:
   p=json.load(response)['answers']['grounding']['probabilities']
  result={{'id': c['id'], 'p_supported': p['supports'], 'p_contradicts': p['contradicts'], 'latency_ms': 1}}
 except Exception:
  result={{'id': c['id'], 'error': 'provider_unavailable', 'latency_ms': 1}}
 print(json.dumps(result), flush=True)
print(json.dumps({{'bye': {{'attempts': 2}}}}), flush=True)
"#,
            endpoint = format!("{}/v1/systemone", mock.url())
        );
        let path = manifest(
            home.path(),
            &script,
            5.0,
            json!({"provider":"loopback", "origins":[mock.url()], "credential_env":"FIXTURE_KEY"}),
        );
        write_policy(home.path(), &json!({"allow_private_hosts":true, "constraints":[{"kind":"processor_egress","processor":"fixture","provider":"loopback"}]}).to_string());
        let mut edge = configured_under_policy(
            home.path(),
            &path,
            &[("FIXTURE_KEY", Path::new("synthetic-key"))],
        );
        let display = invoke(&mut edge, &labelled_origin());
        assert_eq!(
            outcomes(&display),
            ["shown_unconfirmed", "shown_unconfirmed"],
            "{fault}: {display}"
        );
        let record = invocation(home.path());
        assert!(
            record.to_string().contains("sidecar_item_error"),
            "{record}"
        );
        assert_eq!(record["detail"]["model"]["egress"]["usage"]["attempts"], 2);
        assert!(seen.load(Ordering::SeqCst) > 0);
        assert!(
            !record
                .to_string()
                .contains("provider-text-must-not-be-recorded")
        );
    }
}

#[test]
fn sidecar_uncached_dependencies_fail_offline_before_running_the_script() {
    let home = tempfile::tempdir().unwrap();
    let marker = home.path().join("started");
    let path = manifest(
        home.path(),
        &format!("open({marker:?}, 'w').write('started')\n"),
        10.0,
        json!(false),
    );
    let script_path = home.path().join("checker.py");
    let script = std::fs::read_to_string(&script_path).unwrap().replace(
        "dependencies = []",
        "dependencies = [\"commonmeasure-nonexistent-sidecar-dependency==0.0.0\"]",
    );
    std::fs::write(&script_path, &script).unwrap();
    let mut m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    m["artefact"]["sha256"] = json!(sha256_digest(script.as_bytes()).trim_start_matches("sha256:"));
    std::fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();
    let (ok, report) = check(&path, home.path());
    assert!(
        !ok && report.to_string().contains("sidecar_start_failed"),
        "{report}"
    );
    assert!(!marker.exists());
    assert!(
        !report
            .to_string()
            .contains("commonmeasure-nonexistent-sidecar-dependency"),
        "uv's stderr is discarded"
    );
}

#[test]
fn sidecar_completes_one_process_per_invocation_and_status_starts_none() {
    let home = tempfile::tempdir().unwrap();
    let marker = home.path().join("starts");
    let path = manifest(
        home.path(),
        &format!(
            "import json, sys\nopen({marker:?}, 'a').write('x')\n{HELLO}\nfor line in sys.stdin:\n c=json.loads(line)\n print(json.dumps({{'id': c['id'], 'p_supported': .8, 'p_contradicts': .1, 'latency_ms': 0}}), flush=True)\nprint(json.dumps({{'bye': {{'requests': 2}}}}), flush=True)\n"
        ),
        10.0,
        json!(false),
    );
    let mut edge = configured(home.path(), &path, &[]);
    edge.ask(call("context_status", json!({})));
    assert!(
        !marker.exists(),
        "status must not spawn even a valid checker"
    );
    let site = labelled_origin();
    for count in 1..=2 {
        invoke(&mut edge, &site);
        assert_eq!(
            invocation(home.path())["detail"]["model"]["bye"]["requests"],
            2
        );
        edge.ask(call("context_status", json!({})));
        assert_eq!(std::fs::read_to_string(&marker).unwrap().len(), count);
    }
}
