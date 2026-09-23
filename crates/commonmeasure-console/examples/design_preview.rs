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
    serve(ServeOptions {
        listen: format!(
            "127.0.0.1:{}",
            std::env::var("CM_DESIGN_PREVIEW_PORT").unwrap_or_else(|_| "4187".to_owned())
        ),
        allow_remote: false,
        home: fixture.path().to_path_buf(),
        providers: vec![
            json!({"name":"Synthetic source with a long operational name", "connected":false}),
        ],
        search: None,
        liveness: None,
    })
}
