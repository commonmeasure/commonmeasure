//! Observed capture from a host's lifecycle hooks.
//!
//! `PostToolUse` fires after the crossing, so this path records what the agent
//! read and cannot refuse it. It needs no credentials and no configuration,
//! which is what makes it the first real evidence a new operator sees.
//!
//! The one rule that outranks completeness: **capture must never break the
//! agent.** Odd input yields no records rather than an error, and the caller
//! exits zero whatever happens.

use chrono::Utc;
use commonmeasure_types::LicenceState;
use commonmeasure_types::canonical::sha256_digest;
use serde::Deserialize;
use serde_json::Value;

use crate::grounding::{self, ToolKind};
use crate::session::{Crossing, CrossingMode};

/// The subset of a hook payload this reads. Every field is optional so one
/// deserialiser handles every event on every host.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HookInput {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub transcript_path: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub hook_event_name: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: Value,
    #[serde(default)]
    pub tool_response: Value,
    /// Present only when this tool call ran inside a subagent: the subagent
    /// kind (`general-purpose`, `Explore`, …). The main agent's calls carry
    /// neither this nor `agent_id`. It is the host's own discriminator, which
    /// is why subagent attribution is recorded rather than guessed.
    #[serde(default)]
    pub agent_type: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    /// The host's identifier for the current turn. Claude Code sends it as
    /// `prompt_id` on every hook; Codex sends `turn_id`. Either is carried
    /// as the record's `turn_id`; nothing else about the turn is read.
    #[serde(default)]
    pub prompt_id: Option<String>,
    #[serde(default)]
    pub turn_id: Option<String>,
    /// `SessionStart` only: how the session began (`startup`, `resume`,
    /// `clear`, `compact`), as the host names it. Recorded on the nudge
    /// issuance so a reader can see which starts re-delivered it.
    #[serde(default)]
    pub source: Option<String>,
    /// `UserPromptSubmit` only: the prompt text as submitted. Read for the
    /// URLs it names and the statements it embeds (`crate::prompt`); never
    /// recorded.
    #[serde(default)]
    pub prompt: Option<String>,
    /// `SessionEnd` only: the host's word for why the session ended
    /// (Claude Code sends `clear`, `logout`, `prompt_input_exit` or
    /// `other`). Recorded as given on `session_ended`.
    #[serde(default)]
    pub reason: Option<String>,
}

/// Fields no Claude Code hook payload carries and other hosts' payloads
/// do: Cursor's (`conversation_id`, `generation_id`, `cursor_version`,
/// `workspace_roots`), the Copilot CLI's camelCase envelope (`sessionId`,
/// `toolName`, `toolArgs`, `toolResult`), and the `timestamp` and
/// `tool_result` of the snake_case envelope the Copilot CLI sends to hooks
/// configured under Claude Code's event names and VS Code's Copilot Chat
/// sends to every hook. That snake_case envelope otherwise carries Claude
/// Code's own field names (`session_id`, `hook_event_name`, `tool_name`,
/// `tool_input`), and `timestamp` is the one field every documented
/// Copilot and VS Code payload carries and Claude Code's hooks reference
/// lists for none. Cursor, VS Code and the Copilot CLI load Claude Code's
/// hook files and run their commands with payloads of their own, so a
/// reader told it is reading Claude Code refuses a payload wearing any of
/// these rather than recording something untrue. A payload in Claude Code's
/// own shape from another host is refused on that host's environment
/// instead ([`FOREIGN_ENVIRONMENT`]).
const FOREIGN_MARKERS: [&str; 10] = [
    "conversation_id",
    "generation_id",
    "cursor_version",
    "workspace_roots",
    "sessionId",
    "toolName",
    "toolArgs",
    "toolResult",
    "timestamp",
    "tool_result",
];

/// Environment variables another host sets for every hook it runs, where that
/// host also runs the hook commands Claude Code's registration names. Cursor
/// sets `CURSOR_PROJECT_DIR` and loads Claude Code's hook files as
/// third-party hooks. Gemini CLI sets `GEMINI_SESSION_ID`, and its `gemini
/// hooks migrate` copies Claude Code's hooks into its own settings. The Devin
/// CLI sets `DEVIN_PROJECT_DIR` and runs the hooks in
/// `~/.claude/settings.json` by default. Grok Build sets `GROK_SESSION_ID` and
/// scans the same file by default. Gemini CLI and Grok Build also set
/// `CLAUDE_PROJECT_DIR` for compatibility, so that variable does not identify
/// Claude Code. The Devin CLI sends Claude Code's own payload shape, so for
/// these hosts the environment, not the payload, is what tells the reader it
/// is not reading Claude Code. The reader refuses rather than naming the host
/// from the variable, which would record a host the command was not told.
pub const FOREIGN_ENVIRONMENT: [&str; 4] = [
    "CURSOR_PROJECT_DIR",
    "GEMINI_SESSION_ID",
    "DEVIN_PROJECT_DIR",
    "GROK_SESSION_ID",
];

impl HostSurface {
    /// The first of [`FOREIGN_ENVIRONMENT`] set in this process's environment,
    /// when the reader for this surface must refuse under it. Cursor's reader
    /// runs under Cursor's own variable, and the Copilot CLI's reader accepts
    /// only the CLI's own shape, so neither refuses on the environment.
    pub fn foreign_environment(self, is_set: impl Fn(&str) -> bool) -> Option<&'static str> {
        if matches!(self, HostSurface::Cursor | HostSurface::CopilotCli) {
            return None;
        }
        FOREIGN_ENVIRONMENT
            .into_iter()
            .find(|variable| is_set(variable))
    }
}

impl HookInput {
    /// The host's turn identifier under whichever name the host uses.
    pub fn turn_id(&self) -> Option<&str> {
        self.prompt_id.as_deref().or(self.turn_id.as_deref())
    }

    /// One host's payload in the shape the capture path reads, or nothing
    /// when the payload is not that host's. Claude Code's payload is read as
    /// sent, unless it carries another host's markers. Cursor's payload is
    /// mapped field by field: `conversation_id` is the session,
    /// `generation_id` the turn, `tool_output` the response, and Cursor's
    /// event names become the ones the capture path matches on. The Copilot
    /// CLI's camelCase payload is mapped the same way: `sessionId` is the
    /// session and `toolResult.textResultForLlm` the response.
    pub fn from_payload(surface: HostSurface, raw: &Value) -> Option<Self> {
        match surface {
            HostSurface::Cursor => Self::from_cursor(raw),
            HostSurface::CopilotCli => Self::from_copilot(raw),
            _ if Self::is_foreign(surface, raw) => None,
            _ => serde_json::from_value(raw.clone()).ok(),
        }
    }

    /// Whether a payload handed to a reader for `surface` is another host's:
    /// it carries a field only Cursor's, VS Code's or the Copilot CLI's
    /// payloads carry. Distinct from a payload that is merely unreadable,
    /// because the two are answered differently at session start: an
    /// unreadable Claude Code payload still gets the nudge, a foreign one
    /// gets nothing. The Copilot CLI reader accepts only its own shape, so
    /// any other JSON object is foreign to it. Cursor's reader maps any
    /// object and refuses nothing.
    pub fn is_foreign(surface: HostSurface, raw: &Value) -> bool {
        match surface {
            HostSurface::Cursor => false,
            HostSurface::CopilotCli => raw.is_object() && Self::from_copilot(raw).is_none(),
            _ => raw.as_object().is_some_and(|object| {
                FOREIGN_MARKERS
                    .iter()
                    .any(|marker| object.contains_key(*marker))
            }),
        }
    }

    fn from_cursor(raw: &Value) -> Option<Self> {
        let object = raw.as_object()?;
        let text = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_owned);
        // Cursor hands a tool's input and output to `postToolUse` as JSON
        // text; the capture path reads the structure, so the text is parsed
        // where it is JSON and kept as a string where it is not.
        let structured = |key: &str| match object.get(key) {
            Some(Value::String(encoded)) => {
                serde_json::from_str(encoded).unwrap_or_else(|_| Value::String(encoded.clone()))
            }
            Some(other) => other.clone(),
            None => Value::Null,
        };
        Some(Self {
            session_id: text("conversation_id").or_else(|| text("session_id")),
            transcript_path: text("transcript_path"),
            cwd: text("cwd").or_else(|| {
                object
                    .get("workspace_roots")
                    .and_then(|roots| roots.get(0))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }),
            hook_event_name: text("hook_event_name").map(|event| {
                match event.as_str() {
                    "postToolUse" => "PostToolUse",
                    "beforeSubmitPrompt" => "UserPromptSubmit",
                    "sessionStart" => "SessionStart",
                    "stop" => "Stop",
                    "sessionEnd" => "SessionEnd",
                    other => other,
                }
                .to_owned()
            }),
            tool_name: text("tool_name"),
            tool_input: structured("tool_input"),
            tool_response: structured("tool_output"),
            agent_type: None,
            agent_id: None,
            prompt_id: None,
            turn_id: text("generation_id"),
            source: None,
            prompt: text("prompt"),
            reason: text("reason"),
        })
    }

    /// The Copilot CLI's camelCase payload, the format it sends to hooks
    /// configured under camelCase event names, which is how `install
    /// copilot` registers them. Every documented event carries `sessionId`;
    /// a payload without it, or carrying the snake_case envelope's
    /// `session_id` or `tool_name`, or Cursor's `conversation_id`, is not
    /// this shape and is refused. The payload names no event, so the event
    /// is the command argument's.
    fn from_copilot(raw: &Value) -> Option<Self> {
        let object = raw.as_object()?;
        if ["session_id", "tool_name", "conversation_id"]
            .iter()
            .any(|key| object.contains_key(*key))
        {
            return None;
        }
        let text = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_owned);
        let session_id = Some(text("sessionId")?);
        // `toolArgs` is typed `unknown` in the reference; where it arrives
        // as JSON text it is parsed, as Cursor's is.
        let tool_input = match object.get("toolArgs") {
            Some(Value::String(encoded)) => {
                serde_json::from_str(encoded).unwrap_or_else(|_| Value::String(encoded.clone()))
            }
            Some(other) => other.clone(),
            None => Value::Null,
        };
        // `textResultForLlm` is what the model was given, so it is the text
        // a fetch is hashed over.
        let tool_response = object
            .get("toolResult")
            .and_then(|result| result.get("textResultForLlm"))
            .cloned()
            .unwrap_or(Value::Null);
        Some(Self {
            session_id,
            transcript_path: text("transcriptPath"),
            cwd: text("cwd"),
            hook_event_name: None,
            tool_name: text("toolName"),
            tool_input,
            tool_response,
            agent_type: None,
            agent_id: None,
            prompt_id: None,
            turn_id: None,
            source: text("source"),
            prompt: text("prompt"),
            reason: text("reason"),
        })
    }
}

/// Which host observed this. The hosts share the hook envelope, but their
/// observable tools differ, so attribution is explicit rather than inferred
/// from payload shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostSurface {
    ClaudeCode,
    Codex,
    Pi,
    ClaudeDesktop,
    Cursor,
    CopilotCli,
    VsCode,
    /// ChatGPT on the web, read by the browser extension.
    ChatgptWeb,
    /// Google AI Overviews, read by the browser extension.
    GoogleAiOverview,
    /// Bing Copilot Search, read by the browser extension.
    BingCopilotSearch,
    /// Chrome and the Chromium browsers that read the same native messaging
    /// manifest: a registration, never the `host` of a record. The three
    /// surfaces above are what the extension records under.
    Chrome,
}

impl HostSurface {
    pub fn id(self) -> &'static str {
        match self {
            HostSurface::ClaudeCode => "claude-code",
            HostSurface::Codex => "codex",
            HostSurface::Pi => "pi",
            HostSurface::ClaudeDesktop => "claude-desktop",
            HostSurface::Cursor => "cursor",
            HostSurface::CopilotCli => "copilot-cli",
            HostSurface::VsCode => "vscode",
            HostSurface::ChatgptWeb => "chatgpt-web",
            HostSurface::GoogleAiOverview => "google-ai-overview",
            HostSurface::BingCopilotSearch => "bing-copilot-search",
            HostSurface::Chrome => "chrome",
        }
    }

    /// Whether this is a browser answer surface, whose observed crossings
    /// come from the extension (`crate::browser`) rather than a hook payload.
    pub fn is_browser(self) -> bool {
        matches!(
            self,
            HostSurface::ChatgptWeb
                | HostSurface::GoogleAiOverview
                | HostSurface::BingCopilotSearch
        )
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "claude-code" | "claude" => Some(HostSurface::ClaudeCode),
            "codex" => Some(HostSurface::Codex),
            "pi" => Some(HostSurface::Pi),
            "claude-desktop" => Some(HostSurface::ClaudeDesktop),
            "cursor" => Some(HostSurface::Cursor),
            "copilot-cli" | "copilot" => Some(HostSurface::CopilotCli),
            "vscode" => Some(HostSurface::VsCode),
            "chatgpt-web" => Some(HostSurface::ChatgptWeb),
            "google-ai-overview" => Some(HostSurface::GoogleAiOverview),
            "bing-copilot-search" => Some(HostSurface::BingCopilotSearch),
            "chrome" => Some(HostSurface::Chrome),
            _ => None,
        }
    }
}

/// Map one hook payload to the crossings it evidences.
///
/// Pure: the caller appends. A `WebFetch` yields one crossing whose bytes
/// entered context; a `WebSearch` yields one per result URL, grounded false,
/// because the model saw titles and snippets rather than pages.
///
/// `internal_prefixes` is the operator's named exception to the privacy
/// floor (`policy.json`, `record_internal_prefixes`): crossings matching a
/// listed prefix are recorded like public traffic, everything else private
/// stays out. Pass an empty slice for the default floor.
pub fn capture(
    input: &HookInput,
    surface: HostSurface,
    internal_prefixes: &[String],
) -> Vec<Crossing> {
    if input.hook_event_name.as_deref() != Some("PostToolUse") {
        return Vec::new();
    }
    let session_id = input
        .session_id
        .clone()
        .unwrap_or_else(|| "unknown-session".to_owned());
    let tool = input.tool_name.clone().unwrap_or_default();
    // The Copilot CLI names its built-in web tools `web_fetch` and
    // `web_search` (its hooks reference maps them to Claude Code's
    // `WebFetch` and `WebSearch`). The mapping is this host's alone, and the
    // record keeps the host's own name.
    let kind = match (surface, tool.as_str()) {
        (HostSurface::CopilotCli, "web_fetch") => ToolKind::WebFetch,
        (HostSurface::CopilotCli, "web_search") => ToolKind::WebSearch,
        _ => ToolKind::classify(&tool),
    };

    let crossing =
        |url: &str, hash: Option<String>, tokens: Option<u64>, grounded: bool| Crossing {
            session_id: session_id.clone(),
            timestamp: Utc::now(),
            mode: CrossingMode::Observed,
            host: surface.id().to_owned(),
            client: None,
            tool: Some(tool.clone()),
            agent_type: input.agent_type.clone(),
            agent_id: input.agent_id.clone(),
            turn_id: input.turn_id().map(str::to_owned),
            cwd: input.cwd.clone(),
            url: url.to_owned(),
            host_name: grounding::host_of(url),
            internal: grounding::matches_internal_prefix(url, internal_prefixes),
            content_hash: hash,
            retrieved_hash: None,
            estimated_tokens: tokens,
            grounded,
            // No host tool reports rights, and accessibility is not permission.
            licence: LicenceState::Unknown,
            refusal: None,
            // Observed capture cannot refuse and does not judge: policy rulings
            // belong to the mediated path, which is the only one that can act
            // on them before the crossing. No scope governed this either — it
            // had already happened — so none is claimed.
            policy_scope: None,
            principal: None,
            authentication_basis: None,
            breach: None,
            derived_from: None,
            http_status: None,
            failure: None,
            declarations: None,
            named_by: None,
            manifest_record: None,
            content_telemetry_id: None,
            supplier: None,
            identity: None,
            challenge: None,
            allowance: None,
        };

    match kind {
        ToolKind::WebFetch => {
            let Some(url) = input.tool_input.get("url").and_then(Value::as_str) else {
                return Vec::new();
            };
            if !grounding::recordable_under(url, internal_prefixes) {
                return Vec::new();
            }
            let text = grounding::ingested_text(&input.tool_response);
            vec![crossing(
                url,
                Some(sha256_digest(text.as_bytes())),
                Some(grounding::estimate_tokens(&text)),
                true,
            )]
        }
        // Search and MCP results are retrieved, not grounded: nothing here
        // ties a returned URL to bytes that entered context, and claiming
        // otherwise would put a citation in the record with nothing behind it.
        ToolKind::WebSearch | ToolKind::Mcp => grounding::extract_urls(&input.tool_response)
            .into_iter()
            .filter(|url| grounding::recordable_under(url, internal_prefixes))
            .map(|url| crossing(&url, None, None, false))
            .collect(),
        // Already recorded by the server that carried it, before it happened.
        ToolKind::SelfMediated | ToolKind::Other => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn input(event: &str, tool: &str, tool_input: Value, response: Value) -> HookInput {
        HookInput {
            session_id: Some("s-1".into()),
            hook_event_name: Some(event.into()),
            tool_name: Some(tool.into()),
            tool_input,
            tool_response: response,
            ..HookInput::default()
        }
    }

    #[test]
    fn a_webfetch_is_recorded_as_grounded_with_the_hash_of_what_entered_context() {
        let crossings = capture(
            &input(
                "PostToolUse",
                "WebFetch",
                json!({"url": "https://www.ofgem.gov.uk/energy-price-cap"}),
                json!({"code": 200, "result": "The cap is set quarterly."}),
            ),
            HostSurface::ClaudeCode,
            &[],
        );
        assert_eq!(crossings.len(), 1);
        let crossing = &crossings[0];
        assert!(crossing.grounded);
        assert_eq!(crossing.host_name, "www.ofgem.gov.uk");
        assert_eq!(crossing.mode, CrossingMode::Observed);
        assert_eq!(
            crossing.content_hash.as_deref(),
            Some(sha256_digest(b"The cap is set quarterly.").as_str()),
            "the hash must cover the text that entered context, not the wrapper"
        );
    }

    /// A crossing admitted by a named prefix carries the classification in
    /// the record itself, because egress consults the recorded fact — not the
    /// prefix list as it happens to stand later. Public traffic
    /// carries no marking.
    #[test]
    fn a_prefix_admitted_crossing_is_marked_internal_in_the_record() {
        let prefixes = vec!["https://intranet.example.com/private/".to_owned()];
        let internal = capture(
            &input(
                "PostToolUse",
                "WebFetch",
                json!({"url": "https://intranet.example.com/private/handbook"}),
                json!({"result": "handbook text"}),
            ),
            HostSurface::ClaudeCode,
            &prefixes,
        );
        assert_eq!(internal.len(), 1);
        assert!(internal[0].internal);
        assert_eq!(internal[0].to_record()["internal"], json!(true));

        let public = capture(
            &input(
                "PostToolUse",
                "WebFetch",
                json!({"url": "https://www.gov.uk/x"}),
                json!({"result": "public text"}),
            ),
            HostSurface::ClaudeCode,
            &prefixes,
        );
        assert!(!public[0].internal);
        assert_eq!(
            public[0].to_record().get("internal"),
            None,
            "the marking is serialised only when true"
        );
    }

    /// Every spelling of the page the operator named must be stamped internal,
    /// not only the literal one; otherwise the variants are stamped
    /// `internal=false` and leave the machine. The stamp is what egress
    /// consults, so a spelling must not be able to change it.
    #[test]
    fn every_spelling_of_a_named_internal_url_is_stamped_internal() {
        let prefixes = vec!["https://intranet.example.com/private/".to_owned()];
        for spelling in [
            "https://intranet.example.com:443/private/handbook",
            "HTTPS://INTRANET.EXAMPLE.COM/private/handbook",
            "https://intranet.example.com/./private/handbook",
        ] {
            let crossings = capture(
                &input(
                    "PostToolUse",
                    "WebFetch",
                    json!({"url": spelling}),
                    json!({"result": "handbook text"}),
                ),
                HostSurface::ClaudeCode,
                &prefixes,
            );
            assert_eq!(crossings.len(), 1, "{spelling} should be recorded");
            assert!(
                crossings[0].internal,
                "{spelling} should be marked internal"
            );
        }
    }

    #[test]
    fn a_websearch_records_each_result_but_grounds_none_of_them() {
        let crossings = capture(
            &input(
                "PostToolUse",
                "WebSearch",
                json!({"query": "ofgem price cap"}),
                json!({"result": "See https://www.ofgem.gov.uk/a and https://www.gov.uk/b"}),
            ),
            HostSurface::ClaudeCode,
            &[],
        );
        assert_eq!(crossings.len(), 2);
        assert!(
            crossings
                .iter()
                .all(|c| !c.grounded && c.content_hash.is_none()),
            "a search result is not evidence that a page entered context"
        );
    }

    #[test]
    fn subagent_identity_survives_when_the_host_supplies_it() {
        let mut payload = input(
            "PostToolUse",
            "WebFetch",
            json!({"url": "https://example.com/a"}),
            json!({"result": "text"}),
        );
        payload.agent_type = Some("Explore".into());
        payload.agent_id = Some("agent-7".into());
        let crossing = &capture(&payload, HostSurface::ClaudeCode, &[])[0];
        assert_eq!(crossing.agent_type.as_deref(), Some("Explore"));
        assert_eq!(crossing.agent_id.as_deref(), Some("agent-7"));
    }

    /// The turn identifier is the host's own, under the host's own name for
    /// it, and nothing else about the turn is read: a crossing can be grouped
    /// under the turn that caused it while the question stays in the host's
    /// transcript.
    #[test]
    fn the_hosts_turn_identifier_is_carried_and_absent_when_the_host_sends_none() {
        let mut payload = input(
            "PostToolUse",
            "WebFetch",
            json!({"url": "https://example.com/a"}),
            json!({"result": "text"}),
        );
        assert!(
            capture(&payload, HostSurface::ClaudeCode, &[])[0]
                .turn_id
                .is_none()
        );
        payload.prompt_id = Some("prompt-7".into());
        let crossing = &capture(&payload, HostSurface::ClaudeCode, &[])[0];
        assert_eq!(crossing.turn_id.as_deref(), Some("prompt-7"));
        assert_eq!(crossing.to_record()["turn_id"], json!("prompt-7"));

        let mut codex = input(
            "PostToolUse",
            "mcp__somebody-else__search",
            json!({}),
            json!({"result": "https://www.gov.uk/a"}),
        );
        codex.turn_id = Some("turn-3".into());
        assert_eq!(
            capture(&codex, HostSurface::Codex, &[])[0]
                .turn_id
                .as_deref(),
            Some("turn-3")
        );
    }

    #[test]
    fn a_main_agent_call_claims_no_subagent() {
        let crossing = &capture(
            &input(
                "PostToolUse",
                "WebFetch",
                json!({"url": "https://example.com/a"}),
                json!({"result": "text"}),
            ),
            HostSurface::ClaudeCode,
            &[],
        )[0];
        assert!(crossing.agent_type.is_none() && crossing.agent_id.is_none());
    }

    /// The plugin hooks `mcp__.*`, which includes its own tools, so a mediated
    /// fetch would otherwise land in the log twice: once correctly as mediated,
    /// and once as an observed crossing that scraped every URL out of the page
    /// body it had just returned.
    #[test]
    fn our_own_mediated_tools_are_not_observed_a_second_time() {
        let crossings = capture(
            &input(
                "PostToolUse",
                "mcp__commonmeasure__context_fetch",
                json!({"url": "https://www.ofgem.gov.uk/cap"}),
                json!({"content": [{"type": "text", "text":
                    "{\"url\":\"https://www.ofgem.gov.uk/cap\",\"content\":\"see https://elsewhere.example/x\"}"}]}),
            ),
            HostSurface::ClaudeCode,
            &[],
        );
        assert!(
            crossings.is_empty(),
            "the mediated path already recorded this crossing"
        );
    }

    /// The same exclusion under the name production actually delivers:
    /// Claude Code namespaces a plugin-installed server as
    /// `mcp__plugin_<plugin>_<server>__<tool>`, which is how every session
    /// with the installed plugin spells `context_fetch`. Recognising only the
    /// direct-install spelling would make the hook record every mediated
    /// fetch a second time.
    #[test]
    fn the_plugin_namespaced_spelling_is_not_observed_either() {
        let crossings = capture(
            &input(
                "PostToolUse",
                "mcp__plugin_commonmeasure_commonmeasure__context_fetch",
                json!({"url": "https://www.ofgem.gov.uk/cap"}),
                json!({"content": [{"type": "text", "text":
                    "{\"url\":\"https://www.ofgem.gov.uk/cap\",\"content\":\"see https://elsewhere.example/x\"}"}]}),
            ),
            HostSurface::ClaudeCode,
            &[],
        );
        assert!(
            crossings.is_empty(),
            "the mediated path already recorded this crossing"
        );
    }

    /// Another server's MCP tools are still observed.
    #[test]
    fn a_third_party_mcp_result_is_still_captured() {
        let crossings = capture(
            &input(
                "PostToolUse",
                "mcp__somebody-else__search",
                json!({}),
                json!({"result": "https://www.gov.uk/a"}),
            ),
            HostSurface::ClaudeCode,
            &[],
        );
        assert_eq!(crossings.len(), 1);
        assert!(!crossings[0].grounded);
    }

    /// Another PLUGIN's MCP tools are still observed: the self exclusion
    /// matches this plugin's namespace, not the `mcp__plugin_` convention.
    #[test]
    fn a_third_party_plugin_mcp_result_is_still_captured() {
        let crossings = capture(
            &input(
                "PostToolUse",
                "mcp__plugin_somebody_else__search",
                json!({}),
                json!({"result": "https://www.gov.uk/a"}),
            ),
            HostSurface::ClaudeCode,
            &[],
        );
        assert_eq!(crossings.len(), 1);
        assert!(!crossings[0].grounded);
    }

    #[test]
    fn private_hosts_and_unknown_tools_produce_nothing() {
        assert!(
            capture(
                &input(
                    "PostToolUse",
                    "WebFetch",
                    json!({"url": "http://localhost:3000/secret"}),
                    json!({"result": "text"})
                ),
                HostSurface::ClaudeCode,
                &[]
            )
            .is_empty()
        );
        assert!(
            capture(
                &input("PostToolUse", "Bash", json!({}), json!({"result": "ok"})),
                HostSurface::ClaudeCode,
                &[]
            )
            .is_empty()
        );
    }

    /// The case the internal-supply decision exists for: an internal MCP RAG
    /// whose result URLs the floor would otherwise drop silently. A named prefix
    /// records them; everything private the list does not name stays out of
    /// the record in the same payload.
    #[test]
    fn a_named_internal_prefix_records_rag_results_and_nothing_beside_them() {
        let payload = input(
            "PostToolUse",
            "mcp__corp-rag__search",
            json!({"query": "onboarding policy"}),
            json!({"result": "See https://rag.corp.internal/kb/onboarding and \
                    https://wiki.corp.internal/private and https://www.gov.uk/a"}),
        );
        let floor = capture(&payload, HostSurface::ClaudeCode, &[]);
        assert_eq!(
            floor.iter().map(|c| c.url.as_str()).collect::<Vec<_>>(),
            vec!["https://www.gov.uk/a"],
            "without a named prefix the floor holds unconditionally"
        );

        let prefixes = vec!["https://rag.corp.internal/".to_owned()];
        let recorded = capture(&payload, HostSurface::ClaudeCode, &prefixes);
        assert_eq!(
            recorded.iter().map(|c| c.url.as_str()).collect::<Vec<_>>(),
            vec![
                "https://rag.corp.internal/kb/onboarding",
                "https://www.gov.uk/a",
            ],
            "naming one corpus is not consent for the intranet at large"
        );
        assert!(
            recorded.iter().all(|c| !c.grounded),
            "an internal result is retrieved, not grounded, like any other MCP result"
        );
    }

    /// A WebFetch of a named internal page is recorded with the same grades,
    /// hashes and token estimates as public traffic — that is what makes the
    /// internal heatmap comparable with the open-web one.
    #[test]
    fn a_webfetch_under_a_named_prefix_carries_the_same_evidence_as_public_traffic() {
        let prefixes = vec!["https://docs.corp.internal/".to_owned()];
        let crossings = capture(
            &input(
                "PostToolUse",
                "WebFetch",
                json!({"url": "https://docs.corp.internal/runbook"}),
                json!({"code": 200, "result": "Restart the relay last."}),
            ),
            HostSurface::ClaudeCode,
            &prefixes,
        );
        assert_eq!(crossings.len(), 1);
        assert!(crossings[0].grounded);
        assert_eq!(
            crossings[0].content_hash.as_deref(),
            Some(sha256_digest(b"Restart the relay last.").as_str())
        );
        assert_eq!(crossings[0].estimated_tokens, Some(6));
    }

    /// Capture must not break the agent. A payload this does not understand
    /// yields no records rather than an error.
    /// A Cursor payload is read by the Cursor reader and refused by the
    /// Claude Code one, which the hosts that load Claude Code's hook file
    /// would otherwise feed it.
    #[test]
    fn a_cursor_payload_maps_to_the_capture_shape_and_is_refused_as_claude_code() {
        let raw = json!({
            "conversation_id": "conv-1", "generation_id": "gen-7",
            "hook_event_name": "postToolUse", "cursor_version": "3.20.17",
            "workspace_roots": ["/work/project"], "tool_name": "MCP:fetch_page",
            "tool_input": "{\"url\":\"https://www.example.org/report\"}",
            "tool_output": "{\"content\":[{\"text\":\"see https://www.example.org/report\"}]}",
            "duration": 12
        });
        let input = HookInput::from_payload(HostSurface::Cursor, &raw).expect("Cursor's shape");
        assert_eq!(input.session_id.as_deref(), Some("conv-1"));
        assert_eq!(input.turn_id(), Some("gen-7"));
        assert_eq!(input.cwd.as_deref(), Some("/work/project"));
        assert_eq!(input.hook_event_name.as_deref(), Some("PostToolUse"));
        assert_eq!(input.tool_input["url"], "https://www.example.org/report");
        let crossings = capture(&input, HostSurface::Cursor, &[]);
        assert_eq!(crossings.len(), 1);
        assert_eq!(crossings[0].host, "cursor");
        assert_eq!(crossings[0].session_id, "conv-1");
        assert_eq!(crossings[0].turn_id.as_deref(), Some("gen-7"));
        assert!(
            !crossings[0].grounded,
            "an MCP result is retrieved, not grounded"
        );
        assert!(
            HookInput::from_payload(HostSurface::ClaudeCode, &raw).is_none(),
            "a Claude Code reader must not read Cursor's payload"
        );
        let vscode = json!({"sessionId": "s", "toolName": "web_fetch", "toolArgs": {}});
        assert!(HookInput::from_payload(HostSurface::ClaudeCode, &vscode).is_none());
        let claude = json!({"session_id": "s-1", "hook_event_name": "PostToolUse",
                            "tool_name": "WebFetch", "tool_input": {"url": "https://a.example/"},
                            "tool_response": {"result": "text"}});
        assert!(HookInput::from_payload(HostSurface::ClaudeCode, &claude).is_some());
    }

    /// The Copilot CLI's camelCase `postToolUse` payload maps to the capture
    /// shape: `web_fetch` is a grounded fetch hashed over
    /// `textResultForLlm`, recorded under the host's own tool name. Its
    /// snake_case envelope, and VS Code's, are refused by the Claude Code
    /// reader and by this one.
    #[test]
    fn a_copilot_payload_maps_to_the_capture_shape_and_other_shapes_are_refused() {
        let raw = json!({
            "sessionId": "cop-1", "timestamp": 1789400000000i64, "cwd": "/work/project",
            "toolName": "web_fetch", "toolArgs": {"url": "https://www.example.org/report"},
            "toolResult": {"resultType": "success", "textResultForLlm": "The report text."}
        });
        let input =
            HookInput::from_payload(HostSurface::CopilotCli, &raw).expect("Copilot's shape");
        assert_eq!(input.session_id.as_deref(), Some("cop-1"));
        assert_eq!(input.cwd.as_deref(), Some("/work/project"));
        let mut input = input;
        input.hook_event_name = Some("PostToolUse".into());
        let crossings = capture(&input, HostSurface::CopilotCli, &[]);
        assert_eq!(crossings.len(), 1);
        assert_eq!(crossings[0].host, "copilot-cli");
        assert_eq!(crossings[0].tool.as_deref(), Some("web_fetch"));
        assert!(crossings[0].grounded);
        assert_eq!(
            crossings[0].content_hash.as_deref(),
            Some(sha256_digest(b"The report text.").as_str())
        );
        assert!(
            capture(&input, HostSurface::ClaudeCode, &[]).is_empty(),
            "web_fetch is the Copilot CLI's name, not a tool another host is read as having"
        );
        assert!(HookInput::from_payload(HostSurface::ClaudeCode, &raw).is_none());

        let snake = json!({
            "hook_event_name": "PostToolUse", "session_id": "cop-1",
            "timestamp": "2026-09-14T09:00:00.000Z", "cwd": "/work", "tool_name": "WebFetch",
            "tool_input": {"url": "https://www.example.org/"},
            "tool_result": {"result_type": "success", "text_result_for_llm": "text"}
        });
        assert!(HookInput::from_payload(HostSurface::ClaudeCode, &snake).is_none());
        assert!(HookInput::from_payload(HostSurface::CopilotCli, &snake).is_none());
        let claude = json!({"session_id": "s-1", "hook_event_name": "PostToolUse",
                            "tool_name": "WebFetch", "tool_input": {"url": "https://a.example/"},
                            "tool_response": {"result": "text"}});
        assert!(HookInput::from_payload(HostSurface::CopilotCli, &claude).is_none());
    }

    /// Cursor spells an MCP tool `MCP:<tool>`; our own tools under that
    /// spelling, or bare as Pi registers them, are not observed a second time.
    #[test]
    fn our_own_tools_under_cursors_and_pis_spellings_are_not_observed() {
        for tool in [
            "MCP:context_fetch",
            "context_fetch",
            "MCP:context_status",
            "commonmeasure/context_fetch",
            "commonmeasure_context_search",
            "mcp:commonmeasure:context_status",
        ] {
            let raw = json!({
                "conversation_id": "conv-1", "hook_event_name": "postToolUse",
                "tool_name": tool, "tool_input": {"url": "https://www.example.org/"},
                "tool_output": "{\"url\":\"https://www.example.org/\"}"
            });
            let input = HookInput::from_payload(HostSurface::Cursor, &raw).unwrap();
            assert!(
                capture(&input, HostSurface::Cursor, &[]).is_empty(),
                "{tool}"
            );
        }
    }

    #[test]
    fn malformed_payloads_yield_nothing_rather_than_failing() {
        assert!(capture(&HookInput::default(), HostSurface::ClaudeCode, &[]).is_empty());
        assert!(
            capture(
                &input(
                    "PostToolUse",
                    "WebFetch",
                    json!("not an object"),
                    Value::Null
                ),
                HostSurface::Codex,
                &[]
            )
            .is_empty()
        );
    }
}
