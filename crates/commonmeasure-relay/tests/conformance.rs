//! Proves projected documents validate against the pinned Content Telemetry
//! v1.0 schemas (`schema/`, see `SOURCE.md`), offline. This is the gate that
//! keeps "consume the standard, do not fork it" honest as the wire types
//! evolve, and it runs with no network access: cross-schema `$ref`s carry the
//! published `$id` URLs and are resolved from the vendored copies.

use std::fs;
use std::path::PathBuf;

use commonmeasure_types::canonical::canonical_json;
use serde_json::{Value, json};

fn schema_value(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema")
        .join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).expect("schema is valid json")
}

struct VendoredResolver;

impl jsonschema::SchemaResolver for VendoredResolver {
    fn resolve(
        &self,
        _root: &Value,
        url: &url::Url,
        _original: &str,
    ) -> Result<std::sync::Arc<Value>, jsonschema::SchemaResolverError> {
        let name = url
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .unwrap_or_default();
        let local = match name {
            "telemetry-session.json" => "telemetry-session.v1.json",
            "telemetry-event.json" => "telemetry-event.v1.json",
            "telemetry-event-batch.json" => "telemetry-event-batch.v1.json",
            "manifest.json" => "manifest.v1.json",
            other => anyhow::bail!("no vendored copy for schema reference {other}"),
        };
        Ok(std::sync::Arc::new(schema_value(local)))
    }
}

fn failures(schema_file: &str, instance: &Value) -> Vec<String> {
    let schema = schema_value(schema_file);
    // Formats are asserted, not annotated: the manifest schema relies on
    // `format: uri` to reject an id that is not a URI, and the standard's
    // fixtures expect that rejection.
    let validator = jsonschema::JSONSchema::options()
        .with_resolver(VendoredResolver)
        .should_validate_formats(true)
        .compile(&schema)
        .expect("compile schema");
    match validator.validate(instance) {
        Ok(()) => Vec::new(),
        Err(errors) => errors
            .map(|error| format!("{error} @ {}", error.instance_path))
            .collect(),
    }
}

fn assert_valid(schema_file: &str, instance: &Value) {
    let messages = failures(schema_file, instance);
    assert!(
        messages.is_empty(),
        "document failed {schema_file} validation:\n{}",
        messages.join("\n")
    );
}

fn crossing(url: &str, grounded: bool) -> Value {
    json!({
        "seq": 0,
        "timestamp": "2026-08-02T10:00:00.000Z",
        "event": "crossing_observed",
        "payload": {
            "session_id": "8b9cf1c2-0000-4000-8000-000000000001",
            "timestamp": "2026-08-02T10:00:00.000Z",
            "mode": "observed",
            "host": "claude-code",
            "tool": "WebFetch",
            "url": url,
            "host_name": "www.example.com",
            "content_hash": grounded.then_some(
                "sha256:aa044138653177b57cacad53fa4ea2be3b2770e7177198820949acad706450de"
            ),
            "estimated_tokens": grounded.then_some(360),
            "token_basis": grounded.then_some("characters/4"),
            "grounded": grounded,
            "licence": {"state": "unknown"},
        },
    })
}

#[test]
fn a_projected_session_validates_against_the_pinned_batch_schema() {
    let records = [
        crossing("https://www.example.com/witnessed", true),
        crossing("https://www.example.com/searched", false),
    ];
    let batches = commonmeasure_relay::project::project_session(
        "8b9cf1c2-0000-4000-8000-000000000001",
        &records,
        &[],
        &|_| true,
    )
    .batches;
    assert!(!batches.is_empty(), "a witnessed session must project");
    for batch in &batches {
        assert_valid(
            "telemetry-event-batch.v1.json",
            &serde_json::to_value(batch).expect("serialise batch"),
        );
    }
}

#[test]
fn a_projected_run_validates_against_the_pinned_batch_schema() {
    let summary = json!({
        "run": {
            "id": "50e8b489-fd92-46d0-bc31-fcb75f4ec2dc",
            "started_at": "2026-08-02T15:52:03Z",
        },
        "plans": [{
            "id": "exa-only",
            "sources": [{
                "admitted": true,
                "url": "https://www.example.com/price-cap",
                "retrieval_rank": 1,
                "tokens": 129,
                "content_hash":
                    "sha256:caa39059d132617b74b16f2344aa34e2de68ff0940edc5bab8aa19fb2f6938df",
                "licence": {"state": "declared", "reference": "agreement-42"},
            }],
        }],
    });
    let batches = commonmeasure_relay::project::project_run(&summary, &[]).expect("project run");
    assert!(!batches.is_empty());
    for batch in &batches {
        assert_valid(
            "telemetry-event-batch.v1.json",
            &serde_json::to_value(batch).expect("serialise batch"),
        );
    }
    let declared = &batches[0].events[0];
    assert_eq!(declared.license_ref.as_deref(), Some("agreement-42"));
}

// ---------------------------------------------------------------------------
// The golden corpus (`conformance/`, repo root): this relay's actual output,
// frozen as files, so the wire has an executable definition shared with the
// receiver. This side proves two things — the corpus is byte-identical to what
// the projection emits today, and every document in it validates against the
// pinned schemas. The hub replays its pinned copy of the same files into its
// real receiver, so a wire break is caught by whichever repo moved first.
// A deliberate wire change regenerates here (`UPDATE_GOLDENS=1`) and re-pins
// there, in that order; `conformance/README.md` is the contract.

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../conformance")
}

/// One corpus document as it is stored: the canonical JSON of the batch,
/// indented for a reader. The member order is the canonical one, taken from
/// the same function every seal uses, so the frozen bytes cannot move because
/// a dependency changed how `serde_json` orders a map.
fn render(batch: &commonmeasure_relay::wire::WireBatch) -> String {
    let value = serde_json::to_value(batch).expect("serialise batch");
    let canonical: Value =
        serde_json::from_str(&canonical_json(&value)).expect("canonical json re-reads");
    format!(
        "{}\n",
        serde_json::to_string_pretty(&canonical).expect("render batch")
    )
}

/// The fixed ledger inputs behind the corpus. Deliberately exercises the whole
/// emitted surface: both event kinds, a derived (non-UUID) session id, an
/// enrolled edge's key id as the agent identifier with the host tool in its
/// namespaced field, a declared licence, the grounding data fields, a
/// timestamp needing normalisation, a mediated fetch carrying a
/// `Content-Telemetry-ID`, and a run projection — plus a refused crossing, a
/// fetch the origin answered 403 and a fetch nothing answered, each of which
/// must leave no trace in the output. Two further documents carry the
/// supplier field: a mediated search served by a supplier, and a run whose
/// plan acquired through one.
fn golden_documents() -> Vec<(&'static str, commonmeasure_relay::wire::WireBatch)> {
    let session_records = [
        json!({
            "event": "edge_identity",
            "payload": {
                "timestamp": "2026-08-20T08:59:00.000Z",
                "host": "claude-code",
                "hub": "https://hub.example",
                "key_id": "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k",
                "standing": "enrolled",
            },
        }),
        json!({
            "event": "crossing_observed",
            "payload": {
                "timestamp": "2026-08-20T09:00:00.000Z",
                "mode": "observed",
                "host": "claude-code",
                "url": "https://www.example.org/report",
                "grounded": true,
                "content_hash":
                    "sha256:aa044138653177b57cacad53fa4ea2be3b2770e7177198820949acad706450de",
                "estimated_tokens": 360,
                "token_basis": "characters/4",
                "licence": {"state": "declared", "reference": "agreement-42"},
            },
        }),
        json!({
            "event": "crossing_mediated",
            "payload": {
                "timestamp": "2026-08-20T09:05:00Z",
                "mode": "mediated",
                "host": "claude-code",
                "url": "https://news.example.net/story?id=7",
                "grounded": false,
                "licence": {"state": "unknown"},
            },
        }),
        json!({
            "event": "crossing_refused",
            "payload": {
                "timestamp": "2026-08-20T09:06:00Z",
                "url": "https://blocked.example.com/paywalled",
            },
        }),
        json!({
            "event": "crossing_mediated",
            "payload": {
                "timestamp": "2026-08-20T09:07:00Z",
                "mode": "mediated",
                "host": "claude-code",
                "url": "https://publisher.example.org/briefing",
                "http_status": 200,
                "content_telemetry_id": "6f1d2c3b-4a5e-4f60-8b7c-9d0e1f2a3b4c",
                "grounded": true,
                "content_hash":
                    "sha256:3b1a6e5c8d9f0a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293",
                "estimated_tokens": 84,
                "token_basis": "characters/4",
                "licence": {"state": "declared", "reference": "https://publisher.example.org/license.xml"},
            },
        }),
        json!({
            "event": "crossing_mediated",
            "payload": {
                "timestamp": "2026-08-20T09:08:00Z",
                "mode": "mediated",
                "host": "claude-code",
                "url": "https://publisher.example.org/members",
                "http_status": 403,
                "grounded": false,
                "licence": {"state": "unknown"},
            },
        }),
        json!({
            "event": "crossing_mediated",
            "payload": {
                "timestamp": "2026-08-20T09:09:00Z",
                "mode": "mediated",
                "host": "claude-code",
                "url": "https://down.example.org/report",
                "failure": "connect down.example.org: connection refused",
                "grounded": false,
                "licence": {"state": "unknown"},
            },
        }),
    ];
    let session = commonmeasure_relay::project::project_session(
        "golden-session-a",
        &session_records,
        &[],
        &|_| true,
    )
    .batches;

    let run_summary = json!({
        "run": {
            "id": "6e0f9b3a-4c1d-4f2e-8a5b-9d7c2e1f0a3b",
            "started_at": "2026-08-20T10:00:00Z",
        },
        "plans": [{
            "id": "golden-plan",
            "sources": [
                {
                    "admitted": true,
                    "url": "https://www.example.org/price-cap",
                    "retrieval_rank": 1,
                    "tokens": 129,
                    "content_hash":
                        "sha256:caa39059d132617b74b16f2344aa34e2de68ff0940edc5bab8aa19fb2f6938df",
                    "licence": {"state": "declared", "reference": "agreement-42"},
                },
                {
                    "admitted": false,
                    "url": "https://rejected.example.com/alternative",
                    "retrieval_rank": 2,
                },
            ],
        }],
    });
    let run =
        commonmeasure_relay::project::project_run(&run_summary, &[]).expect("project golden run");

    // A mediated search whose results a supplier served: each result is its
    // own crossing naming the supplier, carries no status (no page was
    // fetched) and is not grounded (a snippet, not page text, entered
    // context). The supplier's name is the wire fact this document freezes.
    let supplied_records = [
        json!({
            "event": "edge_identity",
            "payload": {
                "timestamp": "2026-09-09T20:29:00.000Z",
                "host": "claude-code",
                "hub": "https://hub.example",
                "key_id": "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k",
                "standing": "enrolled",
            },
        }),
        json!({
            "event": "crossing_mediated",
            "payload": {
                "timestamp": "2026-09-09T20:29:30Z",
                "mode": "mediated",
                "host": "claude-code",
                "url": "https://www.publisher.example/news/candidates-on-transport",
                "supplier": "ozone",
                "grounded": false,
                "licence": {"state": "unknown"},
            },
        }),
        json!({
            "event": "crossing_mediated",
            "payload": {
                "timestamp": "2026-09-09T20:29:30Z",
                "mode": "mediated",
                "host": "claude-code",
                "url": "https://www.another.example/uk-news/village-loses-bus-route",
                "supplier": "ozone",
                "grounded": false,
                "licence": {"state": "unknown"},
            },
        }),
    ];
    let supplied = commonmeasure_relay::project::project_session(
        "golden-session-b",
        &supplied_records,
        &[],
        &|_| true,
    )
    .batches;

    // A run whose plan acquired through a supplier: the plan's provider is
    // the supplier of every source it admitted, on both event kinds.
    let supplied_run_summary = json!({
        "run": {
            "id": "0f6e0f55-5a6f-4a7c-9d3e-2c1b8a7e6d50",
            "started_at": "2026-09-09T20:29:28Z",
        },
        "plans": [{
            "id": "ozone-only",
            "provider": "ozone",
            "capability": "search",
            "sources": [{
                "admitted": true,
                "url": "https://www.publisher.example/news/candidates-on-transport",
                "retrieval_rank": 1,
                "tokens": 176,
                "content_hash":
                    "sha256:551e74964f4a499d5c16850a6d62afe1f807d15c8ab93f3a30cdd33e2ba411e3",
                "licence": {"state": "unknown"},
            }],
        }],
    });
    let supplied_run = commonmeasure_relay::project::project_run(&supplied_run_summary, &[])
        .expect("project supplied run");

    // A session policy refused twice and admitted once: the batch carries
    // the count and nothing about the two refused sources.
    let refused_records = [
        json!({
            "event": "crossing_refused",
            "payload": {
                "timestamp": "2026-08-21T14:00:00Z",
                "mode": "mediated",
                "host": "codex",
                "url": "https://www.lexisnexis.example/research",
                "grounded": false,
                "refusal": "access rule 3 requires licence \"firm/subscription\" for host www.lexisnexis.example, and no supplier declared one.",
                "licence": {"state": "unknown"},
            },
        }),
        json!({
            "event": "crossing_mediated",
            "payload": {
                "timestamp": "2026-08-21T14:01:00Z",
                "mode": "mediated",
                "host": "codex",
                "url": "https://www.legislation.example/ukpga/1954/56",
                "http_status": 200,
                "grounded": true,
                "content_hash":
                    "sha256:9c56cc51b374c3ba189210d5b6d4bf57790d351c96c47c02190ecf1e430635ab",
                "estimated_tokens": 5120,
                "token_basis": "characters/4",
                "licence": {"state": "unknown"},
            },
        }),
        json!({
            "event": "crossing_refused",
            "payload": {
                "timestamp": "2026-08-21T14:02:00Z",
                "mode": "mediated",
                "host": "codex",
                "url": "https://news.example/paywalled",
                "http_status": 200,
                "grounded": false,
                "refusal": "The licence https://news.example/license.xml permits AI input under payment type subscription, and this edge holds no settlement rail, so the payment term is unmet.",
                "licence": {"state": "declared", "reference": "https://news.example/license.xml"},
            },
        }),
    ];
    let refused = commonmeasure_relay::project::project_session(
        "firm-session-2",
        &refused_records,
        &[],
        &|_| true,
    )
    .batches;

    let [session_batch] = <[_; 1]>::try_from(session).expect("one session batch");
    let [run_batch] = <[_; 1]>::try_from(run).expect("one run batch");
    let [supplied_batch] = <[_; 1]>::try_from(supplied).expect("one supplied session batch");
    let [supplied_run_batch] = <[_; 1]>::try_from(supplied_run).expect("one supplied run batch");
    let [refused_batch] = <[_; 1]>::try_from(refused).expect("one refused session batch");
    assert_eq!(refused_batch.refused, Some(2));
    assert_eq!(
        refused_batch.events.len(),
        2,
        "one admitted crossing, retrieved and grounded"
    );
    vec![
        ("session-grounded.json", session_batch),
        ("run-licensed.json", run_batch),
        ("session-supplied.json", supplied_batch),
        ("run-supplied.json", supplied_run_batch),
        ("session-refused.json", refused_batch),
    ]
}

#[test]
fn the_golden_corpus_is_this_relays_actual_output() {
    for (name, batch) in golden_documents() {
        let path = corpus_dir().join(name);
        let rendered = render(&batch);
        if std::env::var_os("UPDATE_GOLDENS").is_some() {
            fs::write(&path, &rendered).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        }
        let recorded =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        assert_eq!(
            recorded, rendered,
            "conformance/{name} no longer matches the relay's output. If the wire \
             change is intended: regenerate with UPDATE_GOLDENS=1, then re-pin the \
             hub's copy (conformance/README.md)."
        );
    }
}

#[test]
fn every_golden_document_validates_against_the_pinned_batch_schema() {
    let mut documents = 0;
    for entry in fs::read_dir(corpus_dir()).expect("read conformance/") {
        let path = entry.expect("corpus entry").path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let text =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let document: Value = serde_json::from_str(&text).expect("golden document is json");
        assert_valid("telemetry-event-batch.v1.json", &document);
        documents += 1;
    }
    assert!(documents >= 2, "the corpus has gone missing");
}

/// The validation has teeth: a document claiming a schema version the pin does
/// not cover is refused by the vendored schema itself, which is what makes the
/// pin a contract rather than documentation. The probed version is the
/// withdrawn v0.1 preview — the standard's section 12.1 says the two wire
/// versions do not interoperate, and this asserts our pin agrees.
#[test]
fn the_pinned_schema_rejects_a_foreign_schema_version() {
    let records = [crossing("https://www.example.com/witnessed", true)];
    let batches =
        commonmeasure_relay::project::project_session("s-version", &records, &[], &|_| true)
            .batches;
    let mut document = serde_json::to_value(&batches[0]).expect("serialise batch");
    document["schema_version"] = json!("0.1");
    assert!(
        !failures("telemetry-event-batch.v1.json", &document).is_empty(),
        "schema_version 0.1 must not validate against the v1 pin"
    );
}

// ---------------------------------------------------------------------------
// The discovery manifest (`schema/manifest.v1.json`) and the standard's own
// fixtures for it (`schema/manifest-tests/`). The schema decides the shape;
// the consumer rules of section 8.7 go past it, and the fixtures that pass
// the schema and fail only those rules are held against the runtime's reader
// in `crates/commonmeasure-harness/tests/manifest_fixtures.rs`.

fn manifest_fixtures(kind: &str) -> Vec<(String, Value)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema/manifest-tests")
        .join(kind);
    let mut found: Vec<(String, Value)> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .map(|path| {
            let text = fs::read_to_string(&path).expect("fixture");
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                serde_json::from_str(&text).expect("fixture is JSON"),
            )
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

#[test]
fn every_valid_manifest_fixture_validates_against_the_pinned_manifest_schema() {
    let valid = manifest_fixtures("valid");
    assert!(valid.len() >= 8, "the fixtures have gone missing");
    for (name, document) in valid {
        let messages = failures("manifest.v1.json", &document);
        assert!(
            messages.is_empty(),
            "{name} failed:\n{}",
            messages.join("\n")
        );
    }
}

/// An invalid fixture fails the schema, except the ones the standard itself
/// says pass it: those state so in their description and are rejected by
/// the consumer rules instead.
#[test]
fn every_invalid_manifest_fixture_fails_the_schema_or_is_a_consumer_rule() {
    let invalid = manifest_fixtures("invalid");
    assert!(invalid.len() >= 20, "the fixtures have gone missing");
    let mut consumer_rules = 0;
    for (name, document) in invalid {
        let description = document["_test_description"].as_str().unwrap_or_default();
        let schema_passes = description.starts_with("APPLICATION-LAYER VIOLATION")
            || description.contains("Passes JSON Schema");
        let messages = failures("manifest.v1.json", &document);
        if schema_passes {
            assert!(
                messages.is_empty(),
                "{name} is documented as passing the schema and did not:\n{}",
                messages.join("\n")
            );
            consumer_rules += 1;
        } else {
            assert!(!messages.is_empty(), "{name} validated and must not");
        }
    }
    assert!(
        consumer_rules >= 5,
        "the consumer-rule fixtures have gone missing"
    );
}
