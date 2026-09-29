//! Local visual fixture: synthetic evidence, no provider runner or relay.
//! Run with `cargo run -p commonmeasure-console --example design_preview`.
use anyhow::Result;
use commonmeasure_console::{ServeOptions, serve};
use serde_json::json;

fn main() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let sessions = fixture.path().join("sessions");
    std::fs::create_dir(&sessions)?;
    for (index, (mode, host)) in [
        ("mediated", "alpha.example.test"),
        ("observed", "beta.example.test"),
        ("reconstructed", "gamma.example.test"),
    ]
    .iter()
    .enumerate()
    {
        let id = format!("design-session-{index}");
        let event = json!({"seq":1,"timestamp":"2026-09-16T10:00:00Z","event":format!("crossing_{mode}"),"payload":{
            "session_id":id,"timestamp":"2026-09-16T10:00:00Z","mode":mode,"host":"synthetic-agent",
            "url":format!("https://{host}/a-long-source-title-for-layout-verification"),"host_name":host,
            "cwd":"/synthetic/design-preview","grounded":true,"licence":{"state":"unknown"}
        }});
        std::fs::write(sessions.join(format!("{id}.ndjson")), format!("{event}\n"))?;
    }
    // Explicit synthetic variants exercise the real entry without touching an
    // operator home. Every declaration is discarded when the process exits.
    let variant = std::env::var("CM_DESIGN_PREVIEW_POLICY").unwrap_or_else(|_| "many".into());
    let mut policy = json!({"policy_mode":"observe", "constraints":[], "scopes":[
        {"match":"/synthetic/client", "engagement":"Client work", "policy_mode":"strict"},
        {"match":"/synthetic/design-preview", "engagement":"Research", "allow_telemetry_egress":true},
        {"match":"/synthetic/very-long-directory-name/another-long-component/research-material/working-directory", "policy_mode":"prefer"}
    ]});
    match variant.as_str() {
        "empty" => policy["scopes"] = json!([]),
        "one" => policy["scopes"].as_array_mut().unwrap().truncate(1),
        "many" | "managed" | "malformed" | "absent" | "deployment-error" => {}
        _ => anyhow::bail!("unknown synthetic policy variant: {variant}"),
    }
    if variant != "absent" {
        std::fs::write(
            fixture.path().join("policy.json"),
            if variant == "malformed" {
                "{".into()
            } else {
                policy.to_string()
            },
        )?;
    }
    if variant == "managed" {
        std::fs::write(fixture.path().join("deployment.json"), json!({
            "mode":"managed", "organisation":"synthetic-org", "policy_url":"https://hub.example.test/api/v1/policy/desired",
            "signer":{"key_id":"synthetic", "algorithm":"ed25519", "public_key":"00".repeat(32)}
        }).to_string())?;
    } else if variant == "deployment-error" {
        std::fs::write(fixture.path().join("deployment.json"), "{")?;
    }
    // One provider keyed in the synthetic home's credentials file and one
    // named as set by the launching environment, so the Sources screen shows
    // each origin. The key is synthetic, and the environment is not read.
    std::fs::write(
        fixture.path().join("credentials.env"),
        "# synthetic\nEXA_API_KEY=synthetic-preview-key\n",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(
            fixture.path().join("credentials.env"),
            std::fs::Permissions::from_mode(0o600),
        )?;
    }
    let surface = std::env::var("CM_DESIGN_PREVIEW_SURFACE").unwrap_or_default();
    match surface.as_str() {
        "" => {}
        "empty" => {
            for entry in std::fs::read_dir(&sessions)? {
                std::fs::remove_file(entry?.path())?;
            }
        }
        "details" => {
            let event = json!({"seq":1,"timestamp":"2026-09-27T10:00:00Z","event":"turn_started","payload":{
                "session_id":"synthetic-zero-crossings-with-a-long-complete-identifier", "host":"claude-code",
                "cwd":"/synthetic/long-directory-name/another-long-component/research-material/working-directory"
            }});
            std::fs::write(
                sessions.join("synthetic-zero-crossings-with-a-long-complete-identifier.ndjson"),
                format!("{event}\n"),
            )?;
            let id = "compare-53e820c0-2840-47f4-9ff5-efc952463071";
            // A retained comparison interrupted before its final summary still
            // exposes provider outcomes and supports the offline export.
            let records = [
                json!({"event":"comparison_started", "timestamp":"2026-09-27T10:00:00Z", "payload":{
                    "schema":commonmeasure_harness::compare::SCHEMA, "comparison_id":id,
                    "query":"Synthetic retrieval query", "selected_providers":["exa","tavily","you"],
                    "requested_limit":5, "effective_limit":5, "policy_mode":"observe"
                }}),
                json!({"event":"comparison_provider_started", "payload":{"provider":"exa"}}),
                json!({"event":"comparison_provider_finished", "payload":{"provider":"exa","status":"completed",
                    "results_count":1,"received":1,"refused":0,"latency_ms":42,"elapsed_ms":50,
                    "charge":{"money":{"currency":"USD","micros":0}},
                    "cost":{"money":{"currency":"USD","micros":0}},
                    "results":[{"title":"Synthetic source", "url":"https://example.test/a-long-source-title-for-layout-verification", "host":"example.test"}]
                }}),
                json!({"event":"comparison_provider_started", "payload":{"provider":"tavily"}}),
                json!({"event":"comparison_provider_finished", "payload":{"provider":"tavily","status":"unavailable", "error":"Synthetic supplier unavailable", "results":[]}}),
                json!({"event":"comparison_provider_started", "payload":{"provider":"you"}}),
                json!({"event":"comparison_provider_finished", "payload":{"provider":"you","status":"completed", "results_count":0,"received":0,"refused":0,"results":[]}}),
            ];
            std::fs::create_dir(fixture.path().join("comparisons"))?;
            std::fs::write(
                fixture
                    .path()
                    .join("comparisons")
                    .join(format!("{id}.ndjson")),
                records
                    .iter()
                    .map(|record| format!("{record}\n"))
                    .collect::<String>(),
            )?;
        }
        // Sources refused because they need reporting and the operator has
        // not agreed: the Overview's reporting consent block with a count.
        "consent" => {
            let refused = |seq: u64, host: &str| {
                json!({"seq":seq,"timestamp":"2026-09-29T10:00:00Z","event":"crossing_refused","payload":{
                    "session_id":"design-consent","timestamp":"2026-09-29T10:00:00Z","mode":"mediated",
                    "host":"claude-code","url":format!("https://{host}/licensed-article"),"host_name":host,
                    "cwd":"/synthetic/design-preview","grounded":false,"licence":{"state":"declared"},
                    "refusal":format!("{host} needs reporting"),
                    "declarations":{"reporting":{"telemetry_egress_cleared":false,
                        "consent":{"state":"not_given"},"consent_needed":true,"met":false}}
                }})
            };
            std::fs::write(
                sessions.join("design-consent.ndjson"),
                [
                    refused(1, "news.publisher.example.test"),
                    refused(2, "news.publisher.example.test"),
                    refused(3, "journal.example.test"),
                ]
                .iter()
                .map(|record| format!("{record}\n"))
                .collect::<String>(),
            )?;
        }
        "error" => {
            std::fs::write(fixture.path().join("enrolment.json"), "{")?;
            std::fs::write(fixture.path().join("policy.json"), "{")?;
        }
        _ => anyhow::bail!("unknown synthetic surface variant: {surface}"),
    }
    serve(ServeOptions {
        listen: format!(
            "127.0.0.1:{}",
            std::env::var("CM_DESIGN_PREVIEW_PORT").unwrap_or_else(|_| "4187".to_owned())
        ),
        allow_remote: false,
        home: fixture.path().to_path_buf(),
        providers: if surface == "empty" {
            vec![]
        } else if surface == "details" {
            vec![
                json!({"name":"exa", "connected":true}),
                json!({"name":"tavily", "connected":true}),
                json!({"name":"you", "connected":true}),
                json!({"name":"Synthetic supplier with a long operational name"}),
            ]
        } else {
            vec![json!({"name":"Synthetic source with a long operational name", "connected":false})]
        },
        launch_environment: vec!["TAVILY_API_KEY".to_owned()],
        search: None,
        liveness: None,
    })
}
