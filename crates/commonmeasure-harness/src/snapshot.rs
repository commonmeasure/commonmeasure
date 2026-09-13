//! Context-budget snapshots at stable session boundaries.
//!
//! The host's context report is an observation with a named basis, never
//! ground truth inferred from transcript size. Claude Code writes the
//! provider's own usage counters into its transcript beside each model call —
//! `input_tokens`, `cache_read_input_tokens`, `cache_creation_input_tokens`,
//! `output_tokens`, with the model and the call's timestamp — so the most
//! recent call is an API-reported measurement of what the model was sent at
//! that boundary.
//!
//! The same transcript is the host's own account of what it assembled into
//! the window: one `attachment` record per piece of scaffolding it injected
//! (the instruction files, the skill listing, the subagent listing, the
//! names of the tools available on demand, the definitions loaded on
//! demand, files it read), and the message blocks and tool results of the
//! conversation. The inventory reads those records, groups them into named
//! categories and estimates each category's footprint as characters/4 over
//! the host's own record text. It is an explicit estimate with its basis on
//! the record, kept apart from the API-reported counters. What neither
//! basis can see — a context limit, the free capacity, the resident tool
//! schemas the host sends in the request rather than the transcript, the
//! memory folded into the system prompt — is recorded as unavailable rather
//! than estimated into existence (`docs/FAIL-POLICY.md` §7).
//!
//! Nothing here copies conversation text: the reader takes counters, the
//! model name, a timestamp, record kinds, tool and skill names and text
//! lengths from each line and drops the rest.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::BufRead;
use std::path::Path;

use serde_json::{Value, json};

use crate::grounding::{TOKEN_BASIS, ToolKind};

/// The named basis every snapshot carries. A number without its basis would
/// read as a tokeniser's measurement of the context window, which this is
/// not: it is what the provider reported reading for one call.
pub const SNAPSHOT_BASIS: &str = "host transcript usage counters: API-reported token usage of \
     the session's most recent model call, read at the Stop boundary";

/// The basis of the category inventory, distinct from the counters: the
/// host's own records of what it put into the window, measured, not
/// reported.
pub const INVENTORY_BASIS: &str = "host transcript records up to the same boundary: each \
     attachment record the host injected into context, measured as the character count of the \
     host's serialised record; each message text block, tool input and tool result, measured \
     as the character count of its text; both divided by four; an estimate, never a \
     tokeniser's count, cumulative since the last compaction";

/// The host categories no basis here can see, named so the record and every
/// reader report them unavailable instead of inventing them. `context_limit`
/// stays first: readers pin it there.
pub const UNAVAILABLE: [&str; 5] = [
    "context_limit",
    "free_capacity",
    "resident_tool_definitions (the built-in tool schemas travel in the request, not the \
     transcript)",
    "memory (auto-memory is part of the system prompt and not separable from it)",
    "cache state per category",
];

/// The inventory categories, in the order the record lists them. Every name
/// but `system_prompt` is present on every inventory, because the transcript
/// shows the host injecting nothing of that kind as clearly as it shows it
/// injecting something; `system_prompt` appears only when the host wrote a
/// prompt snapshot, and is otherwise listed unavailable.
pub const CATEGORY_SYSTEM_PROMPT: &str = "system_prompt";
pub const CATEGORY_INSTRUCTIONS: &str = "instructions";
pub const CATEGORY_SKILLS: &str = "skills";
pub const CATEGORY_AGENTS: &str = "agents";
pub const CATEGORY_TOOL_LISTING: &str = "tool_listing";
pub const CATEGORY_DEFINITIONS_LOADED: &str = "tool_definitions_loaded";
pub const CATEGORY_LOCAL_FILES: &str = "local_files";
/// Attachment records the host injected whose kind this reader does not
/// know. They entered the window, so they are counted; they are not filed
/// under a kind they were not.
pub const CATEGORY_OTHER_HOST_RECORDS: &str = "other_host_records";
pub const CATEGORY_CONVERSATION: &str = "conversation";
pub const CATEGORY_ACQUIRED: &str = "acquired_content";

/// The categories that are host scaffolding rather than the work itself:
/// everything the host assembled around the conversation and the acquired
/// content. The session report attributes growth between two boundaries to
/// this group, to conversation, or to acquired content.
pub const SCAFFOLDING: [&str; 8] = [
    CATEGORY_SYSTEM_PROMPT,
    CATEGORY_INSTRUCTIONS,
    CATEGORY_SKILLS,
    CATEGORY_AGENTS,
    CATEGORY_TOOL_LISTING,
    CATEGORY_DEFINITIONS_LOADED,
    CATEGORY_LOCAL_FILES,
    CATEGORY_OTHER_HOST_RECORDS,
];

/// The attachment kinds that are the host's standing instructions to the
/// model: instruction files, the environment, model and session notices,
/// and the reminders it repeats. Named so an unknown kind is never filed
/// here by default.
const INSTRUCTION_KINDS: [&str; 14] = [
    "instructions",
    "environment",
    "model",
    "session_context",
    "date",
    "date_change",
    "auto_mode",
    "command_permissions",
    "total_tokens_reminder",
    "batching_reminder_sent",
    "silent_turn_reminder",
    "bash_output_audience_note",
    "read_truncation_notice",
    "remote_session_change",
];

const ALWAYS_PRESENT: [&str; 9] = [
    CATEGORY_INSTRUCTIONS,
    CATEGORY_SKILLS,
    CATEGORY_AGENTS,
    CATEGORY_TOOL_LISTING,
    CATEGORY_DEFINITIONS_LOADED,
    CATEGORY_LOCAL_FILES,
    CATEGORY_OTHER_HOST_RECORDS,
    CATEGORY_CONVERSATION,
    CATEGORY_ACQUIRED,
];

/// One category's measured footprint: how many host records fell into it
/// and how many characters of record text they carried.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Footprint {
    pub records: u64,
    pub estimated_chars: u64,
}

impl Footprint {
    fn add(&mut self, chars: usize) {
        self.records += 1;
        self.estimated_chars += chars as u64;
    }

    /// The characters/4 estimate, the same arithmetic every crossing's
    /// `estimated_tokens` uses.
    pub fn estimated_tokens(&self) -> u64 {
        self.estimated_chars.div_ceil(4)
    }

    fn to_value(self) -> Value {
        json!({
            "records": self.records,
            "estimated_chars": self.estimated_chars,
            "estimated_tokens": self.estimated_tokens(),
        })
    }
}

/// What the host assembled into the window, by category, with the three
/// states of a capability kept apart: available on demand (a name the host
/// listed), loaded (a definition that entered context) and invoked (a call
/// the host recorded). A discoverable tool is none of the other two.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextInventory {
    pub categories: BTreeMap<String, Footprint>,
    /// Tools the host listed by name as loadable on demand, skills it
    /// listed, and subagent types it listed. Names only: a listing is not a
    /// definition and not a call.
    pub tools_on_demand: BTreeSet<String>,
    pub skills_on_demand: BTreeSet<String>,
    pub agents_on_demand: BTreeSet<String>,
    /// Tools whose full definition the host recorded entering context on
    /// demand.
    pub definitions_loaded: BTreeSet<String>,
    /// Every tool call the transcript records, by tool name. Kept across
    /// compactions: an invocation is history, not window contents.
    pub invoked: BTreeMap<String, u64>,
    /// Compactions the transcript records before this boundary. Each one
    /// resets every footprint, because the host rebuilt the window.
    pub compactions: u64,
}

impl ContextInventory {
    fn new() -> Self {
        let mut inventory = Self::default();
        inventory.reset_window();
        inventory
    }

    /// The window was rebuilt: every footprint and listing starts again.
    /// Invocation counts survive, because a call that happened still
    /// happened.
    fn reset_window(&mut self) {
        self.categories = ALWAYS_PRESENT
            .iter()
            .map(|name| ((*name).to_owned(), Footprint::default()))
            .collect();
        self.tools_on_demand.clear();
        self.skills_on_demand.clear();
        self.agents_on_demand.clear();
        self.definitions_loaded.clear();
    }

    fn add(&mut self, category: &str, chars: usize) {
        self.categories
            .entry(category.to_owned())
            .or_default()
            .add(chars);
    }

    /// The footprint of one category, in estimated tokens; `None` for a
    /// category this inventory does not carry, which is unknown, not zero.
    pub fn tokens(&self, category: &str) -> Option<u64> {
        self.categories
            .get(category)
            .map(Footprint::estimated_tokens)
    }

    /// The scaffolding categories this inventory carries, in order.
    pub fn scaffolding_carried(&self) -> Vec<&'static str> {
        SCAFFOLDING
            .iter()
            .copied()
            .filter(|name| self.categories.contains_key(*name))
            .collect()
    }

    /// Everything the host assembled around the work: the sum over the
    /// [`SCAFFOLDING`] categories carried. `None` when none is carried.
    pub fn scaffolding_tokens(&self) -> Option<u64> {
        let carried = self.scaffolding_carried();
        if carried.is_empty() {
            return None;
        }
        Some(carried.iter().filter_map(|name| self.tokens(name)).sum())
    }

    /// Categories the transcript could not show, on top of [`UNAVAILABLE`].
    fn unavailable(&self) -> Vec<String> {
        let mut unavailable: Vec<String> = UNAVAILABLE.iter().map(|s| (*s).to_owned()).collect();
        if !self.categories.contains_key(CATEGORY_SYSTEM_PROMPT) {
            unavailable.push(format!(
                "{CATEGORY_SYSTEM_PROMPT} (this host version wrote no prompt snapshot to the \
                 transcript)"
            ));
        }
        unavailable
    }

    pub fn to_value(&self) -> Value {
        json!({
            "basis": INVENTORY_BASIS,
            "token_basis": TOKEN_BASIS,
            "categories": self.categories.iter()
                .map(|(name, footprint)| (name.clone(), footprint.to_value()))
                .collect::<serde_json::Map<String, Value>>(),
            "available_on_demand": {
                "tools": self.tools_on_demand,
                "skills": self.skills_on_demand,
                "agents": self.agents_on_demand,
            },
            "definitions_loaded": self.definitions_loaded,
            "invoked": self.invoked,
            "compactions": self.compactions,
        })
    }

    /// Read an inventory back from a recorded snapshot payload, for the
    /// session report's comparison between boundaries. A payload missing a
    /// counter is not an inventory with a zero in it: it is no inventory.
    pub fn from_payload(payload: &Value) -> Option<Self> {
        let inventory = payload.get("inventory")?;
        let categories = inventory["categories"]
            .as_object()?
            .iter()
            .map(|(name, footprint)| {
                Some((
                    name.clone(),
                    Footprint {
                        records: footprint["records"].as_u64()?,
                        estimated_chars: footprint["estimated_chars"].as_u64()?,
                    },
                ))
            })
            .collect::<Option<BTreeMap<_, _>>>()?;
        let names = |value: &Value| -> BTreeSet<String> {
            value
                .as_array()
                .map(|names| {
                    names
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        Some(Self {
            categories,
            tools_on_demand: names(&inventory["available_on_demand"]["tools"]),
            skills_on_demand: names(&inventory["available_on_demand"]["skills"]),
            agents_on_demand: names(&inventory["available_on_demand"]["agents"]),
            definitions_loaded: names(&inventory["definitions_loaded"]),
            invoked: inventory["invoked"]
                .as_object()
                .map(|calls| {
                    calls
                        .iter()
                        .filter_map(|(name, count)| Some((name.clone(), count.as_u64()?)))
                        .collect()
                })
                .unwrap_or_default(),
            compactions: inventory["compactions"].as_u64()?,
        })
    }
}

/// One context-budget observation, taken from the host's own record of its
/// most recent model call, with the inventory of what the host had assembled
/// by then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextSnapshot {
    /// The model the host named on the call, when it named one.
    pub model: Option<String>,
    /// The transcript line's own timestamp — the host's clock at the call,
    /// distinct from the capture time the session log stamps on the record.
    pub observed_at: Option<String>,
    pub input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub output_tokens: u64,
    pub inventory: ContextInventory,
}

impl ContextSnapshot {
    /// Everything the provider reported reading for the call: the uncached
    /// tokens plus both cache legs. This is arithmetic over reported
    /// counters, not an estimate, and it is the one derived number a
    /// snapshot carries outside the inventory.
    pub fn context_tokens(&self) -> u64 {
        self.input_tokens + self.cache_read_input_tokens + self.cache_creation_input_tokens
    }

    /// The session-log payload. The basis and the unavailable list travel in
    /// the record itself, so a reader who finds one file needs no other
    /// authority to know what the numbers mean and what is missing.
    pub fn to_payload(&self, session_id: &str, host: &str, transcript: &str) -> Value {
        json!({
            "session_id": session_id,
            "host": host,
            "transcript": transcript,
            "basis": SNAPSHOT_BASIS,
            "model": self.model,
            "observed_at": self.observed_at,
            "context_tokens": self.context_tokens(),
            "input_tokens": self.input_tokens,
            "cache_read_input_tokens": self.cache_read_input_tokens,
            "cache_creation_input_tokens": self.cache_creation_input_tokens,
            "output_tokens": self.output_tokens,
            "inventory": self.inventory.to_value(),
            "unavailable": self.inventory.unavailable(),
        })
    }
}

/// The text length of a message block's content: a string, or the text
/// blocks of a list. Measured, never kept.
fn content_chars(content: &Value) -> usize {
    match content {
        Value::String(text) => text.chars().count(),
        Value::Array(blocks) => blocks
            .iter()
            .map(|block| match block {
                Value::String(text) => text.chars().count(),
                Value::Object(_) => block["text"]
                    .as_str()
                    .map(|text| text.chars().count())
                    .unwrap_or(0),
                _ => 0,
            })
            .sum(),
        _ => 0,
    }
}

fn names_of(value: &Value) -> impl Iterator<Item = String> + '_ {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
}

/// What one transcript line carries forward to the next: whether the host
/// has just written a compaction boundary, so the summary message that
/// follows it is the same compaction and not a second one.
#[derive(Default)]
struct Cursor {
    pending: HashMap<String, String>,
    after_boundary: bool,
}

/// One transcript line's contribution to the inventory. `cursor.pending`
/// maps a tool call's id to its tool name, so its result can be filed under
/// the category the tool earns.
fn absorb(inventory: &mut ContextInventory, value: &Value, cursor: &mut Cursor) {
    let pending = &mut cursor.pending;
    match value["type"].as_str() {
        Some("attachment") => {
            let attachment = &value["attachment"];
            let chars = attachment.to_string().chars().count();
            match attachment["type"].as_str() {
                Some("skill_listing") => {
                    inventory.add(CATEGORY_SKILLS, chars);
                    inventory
                        .skills_on_demand
                        .extend(names_of(&attachment["names"]));
                }
                Some("agent_listing_delta") => {
                    inventory.add(CATEGORY_AGENTS, chars);
                    inventory
                        .agents_on_demand
                        .extend(names_of(&attachment["addedTypes"]));
                    for removed in names_of(&attachment["removedTypes"]) {
                        inventory.agents_on_demand.remove(&removed);
                    }
                }
                Some("deferred_tools_delta") => {
                    inventory.add(CATEGORY_TOOL_LISTING, chars);
                    inventory
                        .tools_on_demand
                        .extend(names_of(&attachment["addedNames"]));
                    inventory
                        .tools_on_demand
                        .extend(names_of(&attachment["readdedNames"]));
                    for removed in names_of(&attachment["removedNames"]) {
                        inventory.tools_on_demand.remove(&removed);
                    }
                }
                Some("deferred_tools_record") => {
                    inventory.add(CATEGORY_DEFINITIONS_LOADED, chars);
                    if let Some(entries) = attachment["entries"].as_array() {
                        inventory.definitions_loaded.extend(
                            entries
                                .iter()
                                .filter_map(|entry| entry["name"].as_str())
                                .map(str::to_owned),
                        );
                    }
                }
                // The host's snapshot of the system prompt replaces the
                // previous one rather than adding to it: there is one
                // system prompt in the window.
                Some("prompt_snapshot") => {
                    let chars = content_chars(&attachment["systemPrompt"]);
                    inventory.categories.insert(
                        CATEGORY_SYSTEM_PROMPT.to_owned(),
                        Footprint {
                            records: 1,
                            estimated_chars: chars as u64,
                        },
                    );
                }
                Some("file" | "directory" | "edited_text_file") => {
                    inventory.add(CATEGORY_LOCAL_FILES, chars);
                }
                // A command the person queued is their words, not the
                // host's.
                Some("queued_command") => inventory.add(CATEGORY_CONVERSATION, chars),
                Some(kind) if INSTRUCTION_KINDS.contains(&kind) => {
                    inventory.add(CATEGORY_INSTRUCTIONS, chars);
                }
                _ => inventory.add(CATEGORY_OTHER_HOST_RECORDS, chars),
            }
        }
        Some("user" | "assistant") => {
            // The host writes a boundary line and then the summary message;
            // one compaction, counted once at whichever of the two appears.
            if value["isCompactSummary"] == Value::Bool(true) && !cursor.after_boundary {
                inventory.compactions += 1;
                inventory.reset_window();
            }
            cursor.after_boundary = false;
            let content = &value["message"]["content"];
            let Some(blocks) = content.as_array() else {
                // A usage-only line carries no content and is not a record
                // of the window.
                if content.is_string() {
                    inventory.add(CATEGORY_CONVERSATION, content_chars(content));
                }
                return;
            };
            for block in blocks {
                match block["type"].as_str() {
                    Some("text") => inventory.add(CATEGORY_CONVERSATION, content_chars(block)),
                    Some("tool_use") => {
                        let name = block["name"].as_str().unwrap_or_default().to_owned();
                        if let Some(id) = block["id"].as_str() {
                            pending.insert(id.to_owned(), name.clone());
                        }
                        *inventory.invoked.entry(name).or_default() += 1;
                        inventory.add(
                            CATEGORY_CONVERSATION,
                            block["input"].to_string().chars().count(),
                        );
                    }
                    Some("tool_result") => {
                        let tool = block["tool_use_id"]
                            .as_str()
                            .and_then(|id| pending.get(id))
                            .cloned()
                            .unwrap_or_default();
                        let result = &block["content"];
                        // A definition the host loaded on demand is recorded
                        // in the result of the call that loaded it.
                        if let Some(references) = result.as_array() {
                            inventory.definitions_loaded.extend(
                                references
                                    .iter()
                                    .filter(|r| r["type"].as_str() == Some("tool_reference"))
                                    .filter_map(|r| r["tool_name"].as_str())
                                    .map(str::to_owned),
                            );
                        }
                        let category = match ToolKind::classify(&tool) {
                            ToolKind::WebFetch
                            | ToolKind::WebSearch
                            | ToolKind::Mcp
                            | ToolKind::SelfMediated => CATEGORY_ACQUIRED,
                            ToolKind::Other => CATEGORY_CONVERSATION,
                        };
                        inventory.add(category, content_chars(result));
                    }
                    // Thinking blocks are not counted: the host does not
                    // report whether it sends earlier turns' thinking again.
                    _ => {}
                }
            }
        }
        Some("system") if value["subtype"].as_str() == Some("compact_boundary") => {
            inventory.compactions += 1;
            inventory.reset_window();
            cursor.after_boundary = true;
        }
        _ => {}
    }
}

/// Read the most recent model call's usage counters, and the inventory of
/// what the host had assembled by then, from a Claude Code transcript.
/// `None` when the file cannot be read or no line carries usage — capture is
/// best-effort and a missing snapshot is a missing snapshot, never an error
/// the agent sees.
pub fn from_claude_transcript(path: &Path) -> Option<ContextSnapshot> {
    let file = std::fs::File::open(path).ok()?;
    let mut last = None;
    let mut inventory = ContextInventory::new();
    let mut cursor = Cursor::default();
    for line in std::io::BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        absorb(&mut inventory, &value, &mut cursor);
        // The host writes assistant lines of its own making (a compaction
        // summary, a command's output) with the model named `<synthetic>`
        // and every counter zero. No model was sent anything for them, so
        // they are not a measurement of the window and never become the
        // snapshot (docs/FAIL-POLICY.md §7).
        if value["message"]["model"].as_str() == Some("<synthetic>") {
            continue;
        }
        let usage = &value["message"]["usage"];
        if !usage.is_object() {
            continue;
        }
        // Every counter this record publishes must be present. The record's
        // stated basis is API-reported usage, and a counter the line does
        // not carry must not become an arithmetic zero inside the derived
        // total — that would publish an absent measurement as a reported
        // one (docs/FAIL-POLICY.md §7). A line missing any counter
        // is a line with no usable snapshot, exactly like a line with none.
        let counter = |name: &str| usage[name].as_u64();
        let (Some(input_tokens), Some(cache_read), Some(cache_creation), Some(output_tokens)) = (
            counter("input_tokens"),
            counter("cache_read_input_tokens"),
            counter("cache_creation_input_tokens"),
            counter("output_tokens"),
        ) else {
            continue;
        };
        last = Some(ContextSnapshot {
            model: value["message"]["model"].as_str().map(str::to_owned),
            observed_at: value["timestamp"].as_str().map(str::to_owned),
            input_tokens,
            cache_read_input_tokens: cache_read,
            cache_creation_input_tokens: cache_creation,
            output_tokens,
            inventory: ContextInventory::default(),
        });
    }
    // The inventory is what the host had assembled at the end of the
    // transcript, which is the window the last call was sent with plus the
    // lines written after it; the call's own counters bound it.
    last.map(|snapshot| ContextSnapshot {
        inventory,
        ..snapshot
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn transcript(lines: &[&str]) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().expect("tempfile");
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
        file
    }

    const USAGE: &str = r#"{"type":"assistant","timestamp":"2026-08-06T07:50:33.661Z","message":{"model":"claude-fable-5","usage":{"input_tokens":3,"cache_read_input_tokens":200,"cache_creation_input_tokens":20,"output_tokens":7}}}"#;

    #[test]
    fn the_most_recent_call_wins_and_junk_lines_are_skipped() {
        let file = transcript(&[
            r#"{"type":"user","message":{"role":"user"}}"#,
            "not json at all",
            r#"{"type":"assistant","timestamp":"2026-08-06T07:49:25.172Z","message":{"model":"claude-fable-5","usage":{"input_tokens":2,"cache_read_input_tokens":100,"cache_creation_input_tokens":10,"output_tokens":5}}}"#,
            USAGE,
        ]);
        let snapshot = from_claude_transcript(file.path()).expect("a snapshot");
        assert_eq!(snapshot.model.as_deref(), Some("claude-fable-5"));
        assert_eq!(
            snapshot.observed_at.as_deref(),
            Some("2026-08-06T07:50:33.661Z")
        );
        assert_eq!(snapshot.context_tokens(), 223);
        assert_eq!(snapshot.output_tokens, 7);
    }

    #[test]
    fn a_transcript_without_usage_yields_no_snapshot_rather_than_zeros() {
        let file = transcript(&[r#"{"type":"user","message":{"role":"user"}}"#]);
        assert_eq!(from_claude_transcript(file.path()), None);
        assert_eq!(
            from_claude_transcript(Path::new("/nonexistent/transcript.jsonl")),
            None
        );
    }

    /// A usage object missing a counter the record publishes is a line with
    /// no usable snapshot, never a zero folded into the derived total: the
    /// record's basis is API-reported usage, and an absent measurement must
    /// stay distinguishable from a reported zero.
    #[test]
    fn a_usage_object_missing_a_counter_yields_no_snapshot_rather_than_a_zero_backed_total() {
        let file = transcript(&[
            r#"{"type":"assistant","timestamp":"2026-08-06T07:49:25.172Z","message":{"model":"claude-fable-5","usage":{"input_tokens":2,"cache_creation_input_tokens":10,"output_tokens":5}}}"#,
        ]);
        assert_eq!(
            from_claude_transcript(file.path()),
            None,
            "a missing cache counter must not be published as an arithmetic zero"
        );

        // And an earlier complete line still wins over a later partial one.
        let file = transcript(&[
            r#"{"type":"assistant","timestamp":"2026-08-06T07:49:25.172Z","message":{"model":"claude-fable-5","usage":{"input_tokens":2,"cache_read_input_tokens":100,"cache_creation_input_tokens":10,"output_tokens":5}}}"#,
            r#"{"type":"assistant","timestamp":"2026-08-06T07:50:33.661Z","message":{"model":"claude-fable-5","usage":{"input_tokens":3,"output_tokens":7}}}"#,
        ]);
        let snapshot = from_claude_transcript(file.path()).expect("the complete line's snapshot");
        assert_eq!(snapshot.context_tokens(), 112);
    }

    #[test]
    fn the_payload_names_its_basis_and_what_is_unavailable() {
        let snapshot = ContextSnapshot {
            model: Some("claude-fable-5".into()),
            observed_at: Some("2026-08-06T07:50:33.661Z".into()),
            input_tokens: 3,
            cache_read_input_tokens: 200,
            cache_creation_input_tokens: 20,
            output_tokens: 7,
            inventory: ContextInventory::new(),
        };
        let payload = snapshot.to_payload("s-1", "claude-code", "/tmp/t.jsonl");
        assert_eq!(payload["basis"], SNAPSHOT_BASIS);
        assert_eq!(payload["context_tokens"], 223);
        assert_eq!(payload["unavailable"][0], "context_limit");
        let unavailable = payload["unavailable"].to_string();
        for missing in [
            "free_capacity",
            "resident_tool_definitions",
            "memory",
            "system_prompt",
        ] {
            assert!(
                unavailable.contains(missing),
                "what the basis cannot see is named in the record itself: {missing}"
            );
        }
        assert_eq!(payload["inventory"]["basis"], INVENTORY_BASIS);
        assert_eq!(payload["inventory"]["token_basis"], "characters/4");
    }

    /// The host's own records, in the shapes Claude Code writes, land in the
    /// named categories, and the three states of a capability stay apart:
    /// listed on demand, loaded, invoked.
    #[test]
    fn host_records_are_filed_by_category_and_capability_states_stay_apart() {
        let file = transcript(&[
            r#"{"type":"attachment","attachment":{"type":"instructions","files":[{"path":"/p/CLAUDE.md","content":"Use British English."}]}}"#,
            r#"{"type":"attachment","attachment":{"type":"skill_listing","content":"- design: canvas\n- dataviz: charts","skillCount":2,"names":["design","dataviz"]}}"#,
            r#"{"type":"attachment","attachment":{"type":"agent_listing_delta","addedTypes":["Explore","Plan"],"addedLines":["- Explore: search","- Plan: plan"],"removedTypes":[]}}"#,
            r#"{"type":"attachment","attachment":{"type":"deferred_tools_delta","addedNames":["WebFetch","WebSearch","Monitor"],"addedLines":["WebFetch","WebSearch","Monitor"],"removedNames":["Monitor"]}}"#,
            r#"{"type":"attachment","attachment":{"type":"prompt_snapshot","systemPrompt":["You are Claude Code.","Be brief."]}}"#,
            r#"{"type":"attachment","attachment":{"type":"file","path":"/p/a.txt","content":"twelve chars"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"fetch the cap page"}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"long private reasoning"},{"type":"tool_use","id":"t1","name":"ToolSearch","input":{"query":"select:WebFetch"}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"tool_reference","tool_name":"WebFetch"}]}]}}"#,
            r#"{"type":"attachment","attachment":{"type":"deferred_tools_record","entries":[{"name":"WebFetch","description":"Fetches a URL"}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t2","name":"WebFetch","input":{"url":"https://www.ofgem.gov.uk/cap"}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t2","content":"The cap is set quarterly."}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t3","name":"Bash","input":{"command":"ls"}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t3","content":"a\nb"}]}}"#,
            USAGE,
        ]);
        let inventory = from_claude_transcript(file.path())
            .expect("a snapshot")
            .inventory;

        let records = |name: &str| inventory.categories[name].records;
        assert_eq!(records(CATEGORY_INSTRUCTIONS), 1);
        assert_eq!(records(CATEGORY_SKILLS), 1);
        assert_eq!(records(CATEGORY_AGENTS), 1);
        assert_eq!(records(CATEGORY_TOOL_LISTING), 1);
        assert_eq!(records(CATEGORY_DEFINITIONS_LOADED), 1);
        assert_eq!(records(CATEGORY_LOCAL_FILES), 1);
        assert_eq!(
            inventory.categories[CATEGORY_SYSTEM_PROMPT],
            Footprint {
                records: 1,
                estimated_chars: 29
            },
            "the prompt snapshot is measured over its text, not its JSON"
        );
        // The user prompt, the three tool_use inputs, the ToolSearch result
        // (no text) and the Bash result; the thinking block is not counted.
        assert_eq!(records(CATEGORY_CONVERSATION), 6);
        assert_eq!(
            inventory.categories[CATEGORY_ACQUIRED],
            Footprint {
                records: 1,
                estimated_chars: 25
            },
            "the WebFetch result is acquired content, measured over its text"
        );
        assert_eq!(
            inventory.categories[CATEGORY_ACQUIRED].estimated_tokens(),
            7
        );

        assert_eq!(
            inventory.tools_on_demand,
            ["WebFetch", "WebSearch"].map(String::from).into(),
            "a removed name leaves the listing"
        );
        assert_eq!(
            inventory.skills_on_demand,
            ["design", "dataviz"].map(String::from).into()
        );
        assert_eq!(
            inventory.agents_on_demand,
            ["Explore", "Plan"].map(String::from).into()
        );
        assert_eq!(
            inventory.definitions_loaded,
            ["WebFetch"].map(String::from).into(),
            "WebSearch was listed but never loaded"
        );
        assert_eq!(inventory.invoked["ToolSearch"], 1);
        assert_eq!(inventory.invoked["WebFetch"], 1);
        assert_eq!(inventory.invoked["Bash"], 1);
        assert!(
            !inventory.invoked.contains_key("WebSearch"),
            "listed and loaded are not invoked"
        );
        assert_eq!(inventory.compactions, 0);
        assert!(inventory.scaffolding_tokens().unwrap() > 0);
        assert_eq!(
            inventory.tokens("memory"),
            None,
            "a category not carried is unknown, not zero"
        );

        let payload = ContextSnapshot {
            model: None,
            observed_at: None,
            input_tokens: 0,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            output_tokens: 0,
            inventory: inventory.clone(),
        }
        .to_payload("s", "claude-code", "t");
        assert!(
            !payload["unavailable"].to_string().contains("system_prompt"),
            "a measured system prompt is not listed unavailable"
        );
        assert_eq!(
            ContextInventory::from_payload(&payload).expect("reads back"),
            inventory,
            "the recorded inventory reads back whole"
        );
    }

    /// The host's own record kinds land where they belong: a reminder the
    /// host repeats is an instruction, a command the person queued is
    /// conversation, and a kind this reader has never seen is counted under
    /// its own name rather than filed as an instruction. The lines are the
    /// shapes Claude Code writes.
    #[test]
    fn host_record_kinds_are_filed_by_name_and_unknown_kinds_are_not_instructions() {
        let file = transcript(&[
            r#"{"type":"attachment","attachment":{"type":"bash_output_audience_note","toolUseID":"toolu_01AbCdEfGhIjKlMnOpQrStUv"}}"#,
            r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"and then run the gates","source_uuid":"5b1a0c0e-8d0e-4f0a-9a3b-2c1d4e5f6a7b","commandMode":"prompt","origin":{"kind":"human"},"timestamp":"2026-09-06T06:20:11.000Z"}}"#,
            r#"{"type":"attachment","attachment":{"type":"some_future_kind","payload":"whatever the host adds next"}}"#,
            USAGE,
        ]);
        let inventory = from_claude_transcript(file.path())
            .expect("a snapshot")
            .inventory;
        assert_eq!(inventory.categories[CATEGORY_INSTRUCTIONS].records, 1);
        assert_eq!(inventory.categories[CATEGORY_CONVERSATION].records, 1);
        assert_eq!(
            inventory.categories[CATEGORY_OTHER_HOST_RECORDS].records, 1,
            "an unknown kind is counted, under its own name"
        );
        assert_eq!(
            inventory.categories[CATEGORY_INSTRUCTIONS].estimated_chars,
            r#"{"type":"bash_output_audience_note","toolUseID":"toolu_01AbCdEfGhIjKlMnOpQrStUv"}"#
                .len() as u64,
            "an attachment is measured as its serialised record, which the basis says"
        );
        assert!(INVENTORY_BASIS.contains("serialised record"));
    }

    /// A compaction rebuilds the window: every footprint and listing starts
    /// again, and the count of compactions says why the totals dropped. What
    /// was invoked before it is still history. The host writes a boundary
    /// line and then the summary; the two are one compaction.
    #[test]
    fn a_compaction_resets_the_window_and_keeps_the_invocation_history() {
        let file = transcript(&[
            r#"{"type":"attachment","attachment":{"type":"skill_listing","content":"- design: canvas","skillCount":1,"names":["design"]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"a very long listing of files"}]}}"#,
            r#"{"type":"system","subtype":"compact_boundary","content":"Conversation compacted","isMeta":false}"#,
            r#"{"type":"user","isCompactSummary":true,"message":{"role":"user","content":"summary"}}"#,
            USAGE,
        ]);
        let inventory = from_claude_transcript(file.path())
            .expect("a snapshot")
            .inventory;
        assert_eq!(
            inventory.compactions, 1,
            "the boundary line and its summary are one compaction"
        );
        assert_eq!(inventory.categories[CATEGORY_SKILLS].records, 0);
        assert_eq!(
            inventory.categories[CATEGORY_CONVERSATION],
            Footprint {
                records: 1,
                estimated_chars: 7
            },
            "only the summary is in the rebuilt window"
        );
        assert!(inventory.skills_on_demand.is_empty());
        assert_eq!(inventory.invoked["Bash"], 1);

        // Either marker alone is also one compaction.
        for marker in [
            r#"{"type":"system","subtype":"compact_boundary","content":"Conversation compacted","isMeta":false}"#,
            r#"{"type":"user","isCompactSummary":true,"message":{"role":"user","content":"summary"}}"#,
        ] {
            let file = transcript(&[marker, USAGE]);
            assert_eq!(
                from_claude_transcript(file.path())
                    .expect("a snapshot")
                    .inventory
                    .compactions,
                1
            );
        }
    }

    /// The host's own `<synthetic>` assistant lines carry zero counters
    /// because no model call produced them; the snapshot stays the last
    /// real call.
    #[test]
    fn a_synthetic_host_line_with_zero_counters_is_not_the_snapshot() {
        let file = transcript(&[
            USAGE,
            r#"{"type":"assistant","timestamp":"2026-08-06T07:51:00.000Z","message":{"model":"<synthetic>","usage":{"input_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":0}}}"#,
        ]);
        let snapshot = from_claude_transcript(file.path()).expect("the real call's snapshot");
        assert_eq!(snapshot.model.as_deref(), Some("claude-fable-5"));
        assert_eq!(snapshot.context_tokens(), 223);
        let file = transcript(&[
            r#"{"type":"assistant","timestamp":"2026-08-06T07:51:00.000Z","message":{"model":"<synthetic>","usage":{"input_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":0}}}"#,
        ]);
        assert_eq!(
            from_claude_transcript(file.path()),
            None,
            "a transcript with only synthetic lines has no snapshot, not a zero one"
        );
    }

    /// A real Claude Code transcript (`demo/host-sessions/README.md`), its
    /// text replaced by same-length filler and everything else as the host
    /// wrote it: four turns, one compaction, the host's own record kinds.
    /// The figures are what the host's records give; the reader's filing,
    /// its capability states and its compaction count are pinned against
    /// them rather than against lines written by hand.
    #[test]
    fn the_recorded_real_transcript_is_read_as_the_live_boundary_was() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/recorded/claude-code-transcript.jsonl");
        let snapshot = from_claude_transcript(&fixture).expect("the fixture has counters");
        assert_eq!(snapshot.model.as_deref(), Some("claude-fable-5-1"));
        assert_eq!(snapshot.context_tokens(), 26203);
        assert_eq!(
            snapshot.observed_at.as_deref(),
            Some("2026-09-06T12:44:23.604Z"),
            "the last real call, not the synthetic line after it"
        );
        let inventory = &snapshot.inventory;
        assert_eq!(inventory.compactions, 1, "one /compact, two host markers");
        let records = |name: &str| inventory.categories[name].records;
        assert_eq!(records(CATEGORY_SYSTEM_PROMPT), 1);
        assert_eq!(records(CATEGORY_INSTRUCTIONS), 6);
        assert_eq!(records(CATEGORY_SKILLS), 1);
        assert_eq!(records(CATEGORY_AGENTS), 1);
        assert_eq!(records(CATEGORY_TOOL_LISTING), 2);
        assert_eq!(records(CATEGORY_DEFINITIONS_LOADED), 1);
        assert_eq!(records(CATEGORY_LOCAL_FILES), 0);
        assert_eq!(records(CATEGORY_OTHER_HOST_RECORDS), 1);
        assert_eq!(records(CATEGORY_CONVERSATION), 8);
        assert_eq!(
            records(CATEGORY_ACQUIRED),
            0,
            "the acquired content was in the window the compaction rebuilt"
        );
        assert_eq!(inventory.tokens(CATEGORY_SYSTEM_PROMPT), Some(3096));
        assert_eq!(inventory.tools_on_demand.len(), 67);
        assert_eq!(inventory.skills_on_demand.len(), 13);
        assert_eq!(inventory.agents_on_demand.len(), 5);
        assert_eq!(
            inventory.definitions_loaded,
            [
                "WebFetch",
                "WebSearch",
                "mcp__commonmeasure__context_fetch",
                "mcp__commonmeasure__context_status",
            ]
            .map(String::from)
            .into()
        );
        assert_eq!(
            inventory.invoked,
            [
                ("ToolSearch", 3),
                ("WebFetch", 1),
                ("WebSearch", 1),
                ("mcp__commonmeasure__context_fetch", 1),
                ("mcp__commonmeasure__context_status", 1),
            ]
            .map(|(name, count)| (name.to_owned(), count))
            .into(),
            "invocations survive the compaction; loaded definitions were reset by it"
        );
    }

    /// A recorded payload missing a counter reads back as no inventory, not
    /// as an inventory with a zero in it.
    #[test]
    fn a_payload_missing_a_counter_reads_back_as_no_inventory() {
        let mut payload = ContextSnapshot {
            model: None,
            observed_at: None,
            input_tokens: 0,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            output_tokens: 0,
            inventory: ContextInventory::new(),
        }
        .to_payload("s", "claude-code", "t");
        assert!(ContextInventory::from_payload(&payload).is_some());
        payload["inventory"]["categories"]["skills"]
            .as_object_mut()
            .unwrap()
            .remove("estimated_chars");
        assert_eq!(ContextInventory::from_payload(&payload), None);
    }
}
