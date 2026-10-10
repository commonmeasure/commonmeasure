//! Context-entry and output-association observations for Claude Code, read
//! from its transcript when a turn ends
//! (`docs/contracts/host-integration.md` §Context entry and citation on
//! Claude Code).
//!
//! Claude Code sends no `commonmeasure/observe` request of its own, so its
//! `Stop` hook reads the transcript the host has just finished writing and
//! submits, for each model call, what that call's request carried from the
//! edge and which of those acquisitions its answer named. Each observation
//! goes through [`SessionLog::record_host_observation`], the validation the
//! MCP method applies, into the log of the MCP server the same host process
//! started, which holds the acquisitions.
//!
//! The transcript is the host's record of what it sent, not the bytes on the
//! wire: a representation hash is of the result as the transcript records
//! it, and an output hash of the message as the transcript records it. No
//! page text, output text or quotation leaves this module, only handles,
//! derived identifiers and hashes.

use std::collections::{HashMap, HashSet};
use std::io::BufRead;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use commonmeasure_types::canonical::{canonical_digest, sha256_digest};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::grounding::SELF_TOOL_PREFIXES;
use crate::host_process::HostProcess;
use crate::session::SessionLog;
use crate::session::observations::valid_hash;

/// The namespace the generation and output identifiers are derived in, so
/// that reading one transcript twice names the same generations and a
/// second `Stop` is an identical retry rather than new observations.
const NAMESPACE: Uuid = Uuid::from_u128(0x6c1f_3f0e_9b1d_4c52_a7e0_2d4b_8f61_c203);

/// The most associations one output observation may carry
/// (`docs/contracts/session-evidence.md` §Output-association observations).
const MAX_ASSOCIATIONS: usize = 1024;

/// One `context_fetch` result the transcript holds: its acquisition handle
/// and the URL the result names.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Acquired {
    handle: Uuid,
    url: Option<String>,
}

/// An acquisition a request carried, with the hash of its representation
/// there.
type Entered = (Acquired, String);

/// Where a `context_fetch` result put its acquisition, as far as the
/// transcript establishes it.
enum Delivery {
    /// In the tool result itself, so in every later request until
    /// compaction: a text result, or a file embedded as a resource whose
    /// bytes hash to the result's `content_hash`.
    InResult(Entered),
    /// In a file the result names by local path. The request carries the
    /// path, not the file, so the file enters only where the transcript
    /// shows the host reading it whole and that read's result carries bytes
    /// that hash to the result's `content_hash`.
    Saved(PathBuf, Entered),
    /// Named by the result with no representation the transcript
    /// establishes: retrieved and citable, never entered.
    Retrieved(Acquired),
}

/// An acquisition the edge admitted for one of the host's own tool calls,
/// which the plugin's router answered through `context_fetch`
/// (`plugin/hooks/register.js`), as the edge's record names it: the record
/// carries the host's identifier for the call, so the binding is the edge's
/// and nothing the tool's result says can make or move it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedCall {
    pub handle: Uuid,
    /// The URL that answered, after any redirect.
    pub url: String,
    /// When the edge recorded the crossing. The host writes the call's
    /// result after the router's answer, so a result the transcript records
    /// earlier is not this call's.
    pub recorded_at: DateTime<Utc>,
    pub carried: Carried,
}

/// How a routed call handed its acquisition to the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Carried {
    /// As text, which the router puts in the tool's result.
    Text,
    /// As a file the edge saved at this path, with the hash of its bytes.
    Saved(PathBuf, String),
    /// In a form a routed result cannot carry.
    Other,
}

/// The routed calls the edge's records name, by the host's tool call
/// identifier.
pub type RoutedCalls = HashMap<String, RoutedCall>;

/// One model call as the transcript is read: when it began, what its
/// request carried, how many results the transcript held before it, and the
/// content blocks of its answer.
struct Call {
    started_at: Option<DateTime<Utc>>,
    entered: Vec<Entered>,
    known: usize,
    blocks: Vec<Value>,
}

/// One tool call the transcript records: the tool's name and, for a read of
/// a whole file, the path it read.
struct ToolUse {
    name: String,
    reads: Option<PathBuf>,
}

/// One model call in the transcript and the observations it owes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generation {
    pub generation_id: Uuid,
    pub output_id: Uuid,
    /// When the transcript first recorded the call's answer. A generation
    /// without one is not observed: it cannot be placed after the edge's
    /// opt-in.
    pub started_at: Option<DateTime<Utc>>,
    /// Each acquisition whose representation was in this call's request,
    /// with the hash of that representation.
    pub entered: Vec<(Uuid, String)>,
    /// The hash of the answer's content blocks as the transcript records
    /// them.
    pub output_hash: String,
    /// The acquisitions the answer's text names by URL or by handle, in the
    /// order first named.
    pub cited: Vec<Uuid>,
}

/// A Claude Code transcript as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    /// The model calls whose request and answer the transcript establishes.
    pub generations: Vec<Generation>,
    /// The first line (counted from 1) that is not a transcript record.
    /// Nothing from that line on is established: neither what a later
    /// request carried nor what its answer said. The call whose answer was
    /// being written there and every later call are left out, so damage
    /// yields no context entry or citation rather than a guess.
    pub damaged_line: Option<usize>,
}

/// Read the generations of a Claude Code transcript. `host_session` is the
/// host's session identifier, which keeps the derived identifiers of two
/// transcripts apart. A `WebFetch` result names an acquisition only where
/// `routed` holds the edge's record for that call, recorded before the
/// transcript recorded the result. Subagent lines and the host's synthetic
/// messages are skipped; reading stops at the first line that is not a JSON
/// object ([`Transcript::damaged_line`]).
pub fn read_claude_transcript(
    path: &Path,
    host_session: &str,
    routed: &RoutedCalls,
) -> std::io::Result<Transcript> {
    let reader = std::io::BufReader::new(std::fs::File::open(path)?);
    let mut tools: HashMap<String, ToolUse> = HashMap::new();
    // The edge's results the next request carries; emptied by compaction,
    // which replaces the conversation with a summary.
    let mut window: Vec<Entered> = Vec::new();
    // Every result seen, for a citation of one outside the window.
    let mut known: Vec<Acquired> = Vec::new();
    // The files the edge saved, by the path its result named.
    let mut saved: HashMap<PathBuf, Entered> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut calls: HashMap<String, Call> = HashMap::new();
    let mut after_boundary = false;
    let mut damaged_line = None;
    for (number, line) in reader.lines().enumerate() {
        let line = match line {
            Ok(line) => line,
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                damaged_line = Some(number + 1);
                break;
            }
            Err(error) => return Err(error),
        };
        if line.trim().is_empty() {
            continue;
        }
        let value = match serde_json::from_str::<Value>(&line) {
            Ok(value) if value.is_object() => value,
            _ => {
                damaged_line = Some(number + 1);
                break;
            }
        };
        if value["isSidechain"] == true {
            continue;
        }
        match value["type"].as_str() {
            Some("system") if value["subtype"] == "compact_boundary" => {
                window.clear();
                after_boundary = true;
                continue;
            }
            Some("user" | "assistant") => {}
            _ => continue,
        }
        if value["isCompactSummary"] == true && !after_boundary {
            window.clear();
        }
        after_boundary = false;
        let message = &value["message"];
        let blocks = message["content"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        let written = timestamp(&value);
        if value["type"] == "assistant" {
            for block in blocks {
                if block["type"] == "tool_use"
                    && let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str())
                {
                    tools.insert(
                        id.to_owned(),
                        ToolUse {
                            name: name.to_owned(),
                            reads: whole_file_read(name, &block["input"]),
                        },
                    );
                }
            }
            let Some(id) = message["id"].as_str() else {
                continue;
            };
            if message["model"] == "<synthetic>" {
                continue;
            }
            // The host writes each content block of one answer on its own
            // line. The request is what preceded the first of them; a
            // result written between two of them belongs to the next call.
            let call = calls.entry(id.to_owned()).or_insert_with(|| {
                order.push(id.to_owned());
                Call {
                    started_at: written,
                    entered: window.clone(),
                    known: known.len(),
                    blocks: Vec::new(),
                }
            });
            call.blocks.extend(blocks.iter().cloned());
            continue;
        }
        for block in blocks {
            if block["type"] != "tool_result" || block["is_error"] == true {
                continue;
            }
            let Some((id, tool)) = block["tool_use_id"]
                .as_str()
                .and_then(|id| Some((id, tools.get(id)?)))
            else {
                continue;
            };
            let entered = if let Some(read) = &tool.reads {
                saved_at(&saved, read)
                    .filter(|(_, hash)| carries_bytes(block, hash))
                    .cloned()
            } else {
                let delivery = if tool.name == "WebFetch" {
                    routed
                        .get(id)
                        .filter(|call| written.is_some_and(|at| call.recorded_at <= at))
                        .map(|call| call.delivery(block))
                } else {
                    delivery(&tool.name, block)
                };
                match delivery {
                    Some(Delivery::InResult(entered)) => {
                        known.push(entered.0.clone());
                        Some(entered)
                    }
                    Some(Delivery::Saved(path, entered)) => {
                        known.push(entered.0.clone());
                        saved.insert(path, entered);
                        None
                    }
                    Some(Delivery::Retrieved(acquired)) => {
                        known.push(acquired);
                        None
                    }
                    None => None,
                }
            };
            if let Some(entered) = entered
                && !window
                    .iter()
                    .any(|(acquired, _)| acquired.handle == entered.0.handle)
            {
                window.push(entered);
            }
        }
    }
    if damaged_line.is_some() {
        // The damaged line may have been a block of the answer being
        // written, which would change what that answer cites.
        order.pop();
    }
    let generations = order
        .into_iter()
        .map(|id| {
            let Call {
                started_at,
                entered,
                known: known_then,
                blocks,
            } = calls.remove(&id).expect("each id was entered");
            let text: Vec<&str> = blocks
                .iter()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str())
                .collect();
            let name = |kind: &str| format!("claude-code {kind} {host_session} {id}");
            Generation {
                generation_id: Uuid::new_v5(&NAMESPACE, name("generation").as_bytes()),
                output_id: Uuid::new_v5(&NAMESPACE, name("output").as_bytes()),
                started_at,
                cited: cited(&text.join("\n"), &entered, &known[..known_then]),
                output_hash: canonical_digest(&Value::Array(blocks)),
                entered: entered
                    .into_iter()
                    .map(|(acquired, hash)| (acquired.handle, hash))
                    .collect(),
            }
        })
        .collect();
    Ok(Transcript {
        generations,
        damaged_line,
    })
}

/// When the transcript wrote a line.
fn timestamp(line: &Value) -> Option<DateTime<Utc>> {
    line["timestamp"]
        .as_str()
        .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
        .map(|at| at.with_timezone(&Utc))
}

impl RoutedCall {
    /// Where the call's acquisition went, as for a direct `context_fetch`
    /// answer: a text result's representation is the tool's result as the
    /// transcript records it, the router's answer built from the edge's
    /// text; a saved file's is the hash of its bytes, entered only where a
    /// whole read of its path carries them.
    fn delivery(&self, block: &Value) -> Delivery {
        let acquired = Acquired {
            handle: self.handle,
            url: Some(self.url.clone()),
        };
        match &self.carried {
            Carried::Text => Delivery::InResult((acquired, canonical_digest(&block["content"]))),
            Carried::Saved(path, hash) => Delivery::Saved(path.clone(), (acquired, hash.clone())),
            Carried::Other => Delivery::Retrieved(acquired),
        }
    }
}

/// The path a `Read` call reads whole. A read of some pages or lines does
/// not put the file's bytes in the request, so it names none.
fn whole_file_read(tool: &str, input: &Value) -> Option<PathBuf> {
    let partial = ["offset", "limit", "pages"]
        .iter()
        .any(|key| !input[key].is_null());
    (tool == "Read" && !partial)
        .then(|| input["file_path"].as_str().map(PathBuf::from))
        .flatten()
}

/// Whether a `Read` result carries, as a base64 source, bytes whose SHA-256
/// is `hash`: the form in which the host puts a PDF or image in the request.
/// A successful read says nothing about what it returned. A text result, a
/// note in place of the file, an empty result, or bytes that are not the
/// fetched ones (the file changed, or the host re-encoded it) establish no
/// entry, so the file stays retrieved only.
fn carries_bytes(block: &Value, hash: &str) -> bool {
    block["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|part| part["source"]["type"] == "base64")
        .filter_map(|part| part["source"]["data"].as_str())
        .any(|data| hashes_to(data, hash))
}

/// Whether base64 `data` decodes to bytes whose SHA-256 is `hash`.
fn hashes_to(data: &str, hash: &str) -> bool {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .is_ok_and(|bytes| sha256_digest(&bytes) == hash)
}

/// The saved file a read of `path` read: the path the result named, or
/// the same file reached by another path.
fn saved_at<'a>(saved: &'a HashMap<PathBuf, Entered>, path: &Path) -> Option<&'a Entered> {
    saved.get(path).or_else(|| {
        std::fs::canonicalize(path)
            .ok()
            .and_then(|path| saved.get(&path))
    })
}

/// The acquisition a tool result carries, if it is a `context_fetch` answer
/// from the edge's own server with a handle, and where its representation
/// went. A text result's representation is its recorded content. A file's
/// is the hash of the bytes handed over, the one the edge accepts for a
/// file, and it is in the result only as an embedded resource of those
/// bytes.
fn delivery(tool: &str, block: &Value) -> Option<Delivery> {
    let own = SELF_TOOL_PREFIXES
        .iter()
        .any(|prefix| tool.strip_prefix(prefix) == Some("context_fetch"));
    if !own {
        return None;
    }
    let payload: Value = serde_json::from_str(crate::provenance::payload_text(block)?).ok()?;
    let acquired = Acquired {
        handle: Uuid::parse_str(payload["acquisition_id"].as_str()?).ok()?,
        url: payload["url"].as_str().map(str::to_owned),
    };
    if payload["content"].is_string() {
        let hash = canonical_digest(&block["content"]);
        return Some(Delivery::InResult((acquired, hash)));
    }
    let Some(hash) = payload["content_hash"]
        .as_str()
        .filter(|hash| valid_hash(hash))
    else {
        return Some(Delivery::Retrieved(acquired));
    };
    let embedded = block["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|part| part["type"] == "resource")
        .filter_map(|part| part["resource"]["blob"].as_str())
        .any(|blob| hashes_to(blob, hash));
    let entered = (acquired, hash.to_owned());
    if embedded {
        return Some(Delivery::InResult(entered));
    }
    match payload["path"].as_str() {
        Some(path) => Some(Delivery::Saved(PathBuf::from(path), entered)),
        None => Some(Delivery::Retrieved(entered.0)),
    }
}

/// The acquisitions `text` names. A URL names every acquisition of that URL
/// the request carried, or, where it carried none, the latest one the
/// transcript held before the call: named, but not entered in that
/// generation, so it projects no citation. A handle names its acquisition.
/// Nothing is matched by meaning. `known` stops at the call, so a later
/// `Stop` reading a longer transcript names the same acquisitions.
fn cited(text: &str, entered: &[Entered], known: &[Acquired]) -> Vec<Uuid> {
    let mut named = Vec::new();
    for reference in url_references(text) {
        let in_request: Vec<Uuid> = entered
            .iter()
            .filter(|(acquired, _)| same_resource(acquired.url.as_deref(), &reference))
            .map(|(acquired, _)| acquired.handle)
            .collect();
        if in_request.is_empty() {
            named.extend(
                known
                    .iter()
                    .rev()
                    .find(|acquired| same_resource(acquired.url.as_deref(), &reference))
                    .map(|acquired| acquired.handle),
            );
        } else {
            named.extend(in_request);
        }
    }
    let lower = text.to_ascii_lowercase();
    named.extend(
        known
            .iter()
            .filter(|acquired| lower.contains(&acquired.handle.hyphenated().to_string()))
            .map(|acquired| acquired.handle),
    );
    let mut seen = HashSet::new();
    named.retain(|handle| seen.insert(*handle));
    named
}

/// A URL as a reference compares: parsed, so that case in the host and an
/// empty path do not matter, and without its fragment, which names a part
/// of the same resource.
fn comparable(url: &str) -> Option<String> {
    let mut parsed = url::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    parsed.set_fragment(None);
    Some(parsed.into())
}

fn same_resource(url: Option<&str>, reference: &str) -> bool {
    url.and_then(comparable).as_deref() == Some(reference)
}

/// The HTTP(S) URLs written in `text`, comparable. A URL ends at white
/// space, at a character no URL holds unescaped, or at a closing bracket it
/// did not open, so a Markdown link `[a](https://…)` ends where the link
/// does; trailing sentence punctuation is not part of it.
fn url_references(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = ["https://", "http://"]
        .iter()
        .filter_map(|scheme| rest.find(scheme))
        .min()
    {
        let candidate = &rest[start..];
        let mut depth = 0usize;
        let mut end = candidate.len();
        for (at, character) in candidate.char_indices() {
            let stop = match character {
                '(' => {
                    depth += 1;
                    false
                }
                ')' if depth == 0 => true,
                ')' => {
                    depth -= 1;
                    false
                }
                c => c.is_whitespace() || "<>\"'`{}|\\^[]".contains(c),
            };
            if stop {
                end = at;
                break;
            }
        }
        let url = candidate[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', '*', '_', '~']);
        found.extend(comparable(url));
        rest = &candidate[end.max(1)..];
    }
    found
}

/// How long the `Stop` hook waits for the transcript to hold the turn's
/// final answer. Claude Code runs `Stop` before the answer's line reaches
/// the file (EDG-207: 26 ms behind in the recorded run), and the host waits
/// for the hook, so the bound is short.
pub const ANSWER_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// How often the wait looks at the transcript again.
const ANSWER_POLL: std::time::Duration = std::time::Duration::from_millis(20);

/// Wait until the transcript at `path` holds the turn's final answer, the
/// text `answer` the host's `Stop` sent as `last_assistant_message`, or
/// until `bound` passes.
///
/// The answer is held when the transcript's last model call, read as
/// [`read_claude_transcript`] reads one, is the turn's final call and has
/// text blocks that, trimmed, equal `answer` trimmed: all of them joined by
/// newlines, or the last of them. A call is the final one only when no
/// prompt or tool result follows it in the file and it asked for no tool;
/// equal text alone is not enough. So an answer from an earlier turn,
/// followed by the current turn's prompt, does not stand in for this one,
/// nor does text the model wrote before a tool call whose result is written,
/// and a line not yet whole matches nothing. Every call before the answer
/// precedes it in the file, so once it is held the whole turn is.
///
/// `Err` names why the turn is not established, and the caller then reads
/// nothing from the transcript: the host sent no answer, an answer with no
/// text, which no line can be matched to, or the bound passed first. The
/// turn's generations stay owed to a later `Stop`.
pub fn await_answer(
    path: &Path,
    answer: Option<&str>,
    bound: std::time::Duration,
) -> Result<(), String> {
    let Some(answer) = answer else {
        return Err(
            "the host sent no last_assistant_message, so the transcript cannot be \
                    shown to hold the turn's final answer"
                .to_owned(),
        );
    };
    let answer = answer.trim();
    if answer.is_empty() {
        return Err("the turn's final answer has no text to find in the transcript".to_owned());
    }
    let deadline = std::time::Instant::now() + bound;
    let mut length = None;
    loop {
        // A transcript that has not grown since the last look is not read
        // again.
        let now = std::fs::metadata(path).ok().map(|metadata| metadata.len());
        if now.is_some() && now != length {
            length = now;
            if last_answer(path).is_some_and(|texts| {
                texts.join("\n").trim() == answer
                    || texts.last().is_some_and(|text| text.trim() == answer)
            }) {
                return Ok(());
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "the transcript did not hold the turn's final answer within {} ms",
                bound.as_millis()
            ));
        }
        std::thread::sleep(ANSWER_POLL);
    }
}

/// The text blocks of the transcript's last model call, where that call
/// can be the turn's final one; no text where it cannot; `None` where the
/// file cannot be read.
///
/// A user line after the call, a prompt or a tool result, opens a request
/// the call did not answer, so the call is not the turn's final one and the
/// final one is not yet written. Nor is a call that asked for a tool,
/// whether the file shows the `tool_use` block or only the `tool_use` stop
/// reason Claude Code writes on each of the call's lines. Lines that are
/// not JSON objects are passed over: a line still being written is not an
/// answer, and damage earlier in the file is [`read_claude_transcript`]'s
/// to handle.
fn last_answer(path: &Path) -> Option<Vec<String>> {
    struct Call {
        id: String,
        texts: Vec<String>,
        asked_for_tool: bool,
    }
    let reader = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let mut last: Option<Call> = None;
    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => continue,
            Err(_) => return None,
        };
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value["isSidechain"] == true {
            continue;
        }
        let message = &value["message"];
        if value["type"] == "user" {
            last = None;
            continue;
        }
        if value["type"] != "assistant" || message["model"] == "<synthetic>" {
            continue;
        }
        let Some(id) = message["id"].as_str() else {
            continue;
        };
        if last.as_ref().is_none_or(|last| last.id != id) {
            last = Some(Call {
                id: id.to_owned(),
                texts: Vec::new(),
                asked_for_tool: false,
            });
        }
        if let Some(call) = last.as_mut() {
            let blocks = message["content"].as_array().into_iter().flatten();
            call.asked_for_tool |= message["stop_reason"] == "tool_use"
                || blocks.clone().any(|block| block["type"] == "tool_use");
            call.texts.extend(
                blocks
                    .filter(|block| block["type"] == "text")
                    .filter_map(|block| block["text"].as_str().map(str::to_owned)),
            );
        }
    }
    Some(
        last.filter(|call| !call.asked_for_tool)
            .map(|call| call.texts)
            .unwrap_or_default(),
    )
}

/// What one turn's observations did to one MCP log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recorded {
    pub log: PathBuf,
    pub context_entries: u64,
    pub outputs: u64,
    /// Observations the edge refused as invalid, with its reason: a
    /// duplicate from an earlier `Stop`, or a handle this log never issued.
    pub refused: Vec<String>,
    /// Why recording stopped short, where the log could not be written.
    pub unavailable: Option<String>,
    /// The transcript's first damaged line, from which on nothing was
    /// submitted ([`Transcript::damaged_line`]).
    pub damaged_line: Option<usize>,
}

/// What an MCP log already holds that decides which generations it is owed.
struct Opened {
    started_at: DateTime<Utc>,
    admitted: HashSet<Uuid>,
    /// Generations with an output observation.
    associated: HashSet<Uuid>,
    /// Generations whose completion boundary the log holds: the output was
    /// acknowledged.
    completed: HashSet<Uuid>,
    /// The acquisitions this log issued for a host's own tool call, with
    /// that call's identifier.
    routed: Vec<(String, RoutedCall)>,
}

/// Read an MCP log's opt-in time, the handles it issued and the host calls
/// it issued them for, the generations with an output and the generations it
/// has completed, or `None` where it never opted in for `host`.
fn opened(path: &Path, host: &str) -> Option<Opened> {
    let (records, _) = SessionLog::read_with(path, crate::TornTail::Forecast).ok()?;
    let started = records.iter().find(|record| {
        record["event"] == "observations_started" && record["payload"]["host"] == host
    })?;
    let started_at = DateTime::parse_from_rfc3339(started["payload"]["timestamp"].as_str()?)
        .ok()?
        .with_timezone(&Utc);
    let uuid = |value: &Value| value.as_str().and_then(|id| Uuid::parse_str(id).ok());
    Some(Opened {
        started_at,
        admitted: records
            .iter()
            .filter(|record| record["event"] == "crossing_mediated")
            .filter_map(|record| uuid(&record["payload"]["acquisition_id"]))
            .collect(),
        associated: records
            .iter()
            .filter(|record| record["event"] == "output_associated")
            .filter_map(|record| uuid(&record["payload"]["generation_id"]))
            .collect(),
        completed: records
            .iter()
            .filter(|record| {
                record["event"] == "turn_completed" && record["payload"]["host"] == host
            })
            .filter_map(|record| uuid(&record["payload"]["turn_id"]))
            .collect(),
        routed: records
            .iter()
            .filter(|record| record["event"] == "crossing_mediated")
            .filter_map(|record| routed_call(path, &record["payload"]))
            .collect(),
    })
}

/// The routed call a `crossing_mediated` record names: the host's tool call
/// identifier with the acquisition the edge issued for it, the URL that
/// answered and how the bytes were handed over. A saved file is named by
/// the path the edge saved it under, beside the log, as the edge's result
/// named it.
fn routed_call(log: &Path, payload: &Value) -> Option<(String, RoutedCall)> {
    let host_call = payload["host_call_id"].as_str()?;
    let hash = payload["content_hash"]
        .as_str()
        .filter(|hash| valid_hash(hash));
    let carried = match (payload["delivered_file"]["via"].as_str(), hash) {
        (None, Some(_)) => Carried::Text,
        (Some("local_file"), Some(hash)) => {
            let hex = hash.strip_prefix("sha256:")?;
            let path = crate::fetched_file::saved_at(&crate::fetched_file::directory_for(log), hex);
            Carried::Saved(
                std::fs::canonicalize(&path).unwrap_or(path),
                hash.to_owned(),
            )
        }
        _ => Carried::Other,
    };
    let call = RoutedCall {
        handle: Uuid::parse_str(payload["acquisition_id"].as_str()?).ok()?,
        url: payload["url"].as_str()?.to_owned(),
        recorded_at: DateTime::parse_from_rfc3339(payload["timestamp"].as_str()?)
            .ok()?
            .with_timezone(&Utc),
        carried,
    };
    Some((host_call.to_owned(), call))
}

/// Submit a finished Claude Code turn's observations to every MCP log the
/// host process `process` started that opted into host observations.
///
/// A generation is owed to a log when the transcript recorded it after the
/// log's opt-in and the log holds no completion boundary for it. Its context
/// entries go first, one per acquisition that log issued and the request
/// carried; its output follows, naming every acquisition the answer cited,
/// and a handle the log did not issue stays unresolved there. Where the
/// output was recorded and its boundary was not, only the output is
/// submitted again: the transcript names it identically, and the ingress
/// finishes an identical retry by appending the boundary. A refusal of one
/// observation is kept and the rest continue; a log that cannot be written
/// stops for that log.
pub fn record_claude_turn(
    home: &Path,
    process: &HostProcess,
    transcript: &Path,
    host_session: &str,
    host: &str,
    cwd: Option<&str>,
    policy: Option<&crate::policy::PolicyIdentity>,
) -> std::io::Result<Vec<Recorded>> {
    let logs: Vec<(PathBuf, Opened)> = crate::host_process::logs_of(home, process)?
        .into_iter()
        .filter_map(|path| opened(&path, host).map(|opened| (path, opened)))
        .collect();
    if logs.is_empty() {
        return Ok(Vec::new());
    }
    // A host call named by two acquisitions, in one log or two, is bound to
    // neither: which one its result carried is not established.
    let mut routed: HashMap<String, Option<RoutedCall>> = HashMap::new();
    for (host_call, call) in logs.iter().flat_map(|(_, opened)| &opened.routed) {
        routed
            .entry(host_call.clone())
            .and_modify(|bound| *bound = None)
            .or_insert_with(|| Some(call.clone()));
    }
    let routed: RoutedCalls = routed
        .into_iter()
        .filter_map(|(host_call, call)| Some((host_call, call?)))
        .collect();
    let Transcript {
        generations,
        damaged_line,
    } = read_claude_transcript(transcript, host_session, &routed)?;
    let mut report = Vec::new();
    for (path, opened) in logs {
        let mut recorded = Recorded {
            log: path.clone(),
            damaged_line,
            ..Recorded::default()
        };
        let Some(session_id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let mut log = SessionLog::open(home, session_id)?;
        if let Some(policy) = policy {
            log.set_policy_identity(policy);
        }
        let owed = generations.iter().filter(|generation| {
            generation
                .started_at
                .is_some_and(|at| at >= opened.started_at)
                && !opened.completed.contains(&generation.generation_id)
        });
        'generations: for generation in owed {
            // The ingress takes no context entry for a generation that has
            // an output, so a retry resubmits the output alone.
            let associated = opened.associated.contains(&generation.generation_id);
            let entries = generation
                .entered
                .iter()
                .filter(|_| !associated)
                .filter(|(handle, _)| opened.admitted.contains(handle))
                .map(|(handle, hash)| {
                    json!({"event": "context_entered", "acquisition_id": handle,
                           "generation_id": generation.generation_id,
                           "representation_hash": hash})
                });
            let mut cited = generation.cited.clone();
            cited.truncate(MAX_ASSOCIATIONS);
            let output = json!({"event": "output_associated",
                "generation_id": generation.generation_id, "output_id": generation.output_id,
                "output_hash": generation.output_hash, "acquisition_ids": cited});
            for observation in entries.chain(std::iter::once(output)) {
                let output = observation["event"] == "output_associated";
                match log.record_host_observation(observation, host, cwd) {
                    Ok(_) if output => recorded.outputs += 1,
                    Ok(_) => recorded.context_entries += 1,
                    Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {
                        recorded.refused.push(error.to_string());
                    }
                    Err(error) => {
                        recorded.unavailable = Some(error.to_string());
                        break 'generations;
                    }
                }
            }
        }
        report.push(recorded);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EDGE: &str = "mcp__plugin_commonmeasure_commonmeasure__context_fetch";

    fn fetched(id: &str, handle: Uuid, url: &str) -> Vec<Value> {
        let payload = json!({"url": url, "acquisition_id": handle, "content": format!("text of {url}"),
            "content_hash": format!("sha256:{}", "a".repeat(64))});
        vec![
            json!({"type":"assistant","timestamp":"2026-10-09T10:00:01Z","message":{"id":format!("msg_{id}"),
                "model":"claude-fable-5","content":[{"type":"tool_use","id":id,"name":EDGE,"input":{"url":url}}]}}),
            json!({"type":"user","timestamp":"2026-10-09T10:00:02Z","message":{"role":"user","content":[
                {"type":"tool_result","tool_use_id":id,"content":[
                    {"type":"text","text":"Common Measure · host example.org · …"},
                    {"type":"text","text":payload.to_string()}]}]}}),
        ]
    }

    fn answer(id: &str, text: &str) -> Value {
        json!({"type":"assistant","timestamp":"2026-10-09T10:00:05Z","message":{"id":id,
            "model":"claude-fable-5","content":[{"type":"text","text":text}]}})
    }

    fn transcript(lines: &[Value]) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
        file
    }

    /// The generations of an undamaged transcript of `lines`.
    fn read(lines: &[Value], session: &str) -> Vec<Generation> {
        let read =
            read_claude_transcript(transcript(lines).path(), session, &RoutedCalls::new()).unwrap();
        assert_eq!(read.damaged_line, None);
        read.generations
    }

    /// The generations of an undamaged transcript of `lines`, with the
    /// edge's records of the calls it answered for the host.
    fn read_routed(lines: &[Value], routed: &RoutedCalls) -> Vec<Generation> {
        let read = read_claude_transcript(transcript(lines).path(), "s", routed).unwrap();
        assert_eq!(read.damaged_line, None);
        read.generations
    }

    #[test]
    fn each_call_enters_what_preceded_it_and_its_answer_cites_by_url() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut lines = fetched("t1", a, "https://example.org/a");
        lines.extend(fetched("t2", b, "https://example.org/b"));
        lines.push(answer(
            "msg_answer",
            "The cap is set quarterly ([source](https://Example.org/a#cap)).",
        ));
        let generations = read(&lines, "s");
        assert_eq!(generations.len(), 3);
        assert!(generations[0].entered.is_empty());
        let handles = |g: &Generation| g.entered.iter().map(|(h, _)| *h).collect::<Vec<_>>();
        assert_eq!(handles(&generations[1]), vec![a]);
        assert_eq!(handles(&generations[2]), vec![a, b]);
        assert_eq!(generations[2].cited, vec![a]);
        assert!(generations[0].cited.is_empty());
        // The representation is the recorded result, not the page text.
        let representation = canonical_digest(&lines[1]["message"]["content"][0]["content"]);
        assert_eq!(generations[2].entered[0].1, representation);
        // Reading again names the same generations: a second Stop retries.
        let again = read(&lines, "s");
        assert_eq!(again, generations);
        let other = read(&lines, "other");
        assert_ne!(other[2].generation_id, generations[2].generation_id);
    }

    #[test]
    fn a_handle_names_its_acquisition_and_a_url_prefix_names_nothing() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut lines = fetched("t1", a, "https://example.org/a");
        lines.extend(fetched("t2", b, "https://example.org/b"));
        lines.push(answer(
            "msg_answer",
            &format!("See https://example.org/ab, https://example.org/a/x and acquisition {b}."),
        ));
        let generations = read(&lines, "s");
        assert_eq!(generations[2].cited, vec![b]);
    }

    #[test]
    fn compaction_empties_the_request_and_a_later_citation_names_the_last_known() {
        let a = Uuid::new_v4();
        let mut lines = fetched("t1", a, "https://example.org/a");
        lines.push(json!({"type":"system","subtype":"compact_boundary"}));
        lines.push(json!({"type":"user","isCompactSummary":true,"message":{"role":"user","content":"summary"}}));
        lines.push(answer("msg_answer", "From https://example.org/a."));
        let generations = read(&lines, "s");
        assert!(generations[1].entered.is_empty());
        assert_eq!(generations[1].cited, vec![a]);
    }

    #[test]
    fn errors_other_servers_and_oversized_results_carry_no_acquisition() {
        let a = Uuid::new_v4();
        let mut lines = fetched("t1", a, "https://example.org/a");
        lines[1]["message"]["content"][0]["is_error"] = json!(true);
        let mut other = fetched("t2", Uuid::new_v4(), "https://example.org/b");
        other[0]["message"]["content"][0]["name"] = json!("mcp__elsewhere__context_fetch");
        lines.extend(other);
        let mut stub = fetched("t3", Uuid::new_v4(), "https://example.org/c");
        stub[1]["message"]["content"][0]["content"] = json!(
            "Error: result exceeds maximum allowed tokens. Output has been saved to /tmp/x.txt"
        );
        lines.extend(stub);
        lines.push(answer("msg_answer", "nothing"));
        let generations = read(&lines, "s");
        assert!(generations.last().unwrap().entered.is_empty());
    }

    /// A PDF fetch as the stdio edge answers it: saved under `path`, the
    /// result naming the path and the hash of the bytes.
    fn saved_pdf(handle: Uuid, path: &str, hash: &str) -> Vec<Value> {
        let mut lines = fetched("t1", handle, "https://example.org/a.pdf");
        let payload = json!({"url":"https://example.org/a.pdf","acquisition_id":handle,
            "path":path,"content_hash":hash,"read":"read the file at path"});
        lines[1]["message"]["content"][0]["content"][1]["text"] = json!(payload.to_string());
        lines
    }

    fn read_file(id: &str, input: Value, result: Value) -> Vec<Value> {
        vec![
            json!({"type":"assistant","timestamp":"2026-10-09T10:00:03Z","message":{"id":format!("msg_{id}"),
                "model":"claude-fable-5","content":[{"type":"tool_use","id":id,"name":"Read","input":input}]}}),
            json!({"type":"user","timestamp":"2026-10-09T10:00:04Z","message":{"role":"user","content":[
                {"type":"tool_result","tool_use_id":id,"content":[result]}]}}),
        ]
    }

    #[test]
    fn a_saved_file_never_read_is_retrieved_and_named_but_not_entered() {
        let a = Uuid::new_v4();
        let hash = format!("sha256:{}", "b".repeat(64));
        let mut lines = saved_pdf(a, "/tmp/edge/a.pdf", &hash);
        // A read of some pages, a read of another file and a failed read put
        // none of its bytes in the request.
        lines.extend(read_file(
            "r1",
            json!({"file_path":"/tmp/edge/a.pdf","pages":"1-2"}),
            json!({"type":"text","text":"…"}),
        ));
        lines.extend(read_file(
            "r2",
            json!({"file_path":"/tmp/edge/b.pdf"}),
            json!({"type":"text","text":"…"}),
        ));
        let mut failed = read_file(
            "r3",
            json!({"file_path":"/tmp/edge/a.pdf"}),
            json!({"type":"text","text":"File does not exist."}),
        );
        failed[1]["message"]["content"][0]["is_error"] = json!(true);
        lines.extend(failed);
        lines.push(answer("msg_answer", "See https://example.org/a.pdf."));
        let generations = read(&lines, "s");
        let last = generations.last().unwrap();
        assert!(generations.iter().all(|g| g.entered.is_empty()));
        assert_eq!(last.cited, vec![a]);
    }

    fn document(bytes: &[u8]) -> Value {
        use base64::Engine as _;
        json!({"type":"document","source":{"type":"base64","media_type":"application/pdf",
            "data":base64::engine::general_purpose::STANDARD.encode(bytes)}})
    }

    /// The generations of a transcript in which a saved PDF of `bytes` is
    /// read whole and `result` is what the read returned.
    fn read_saved(bytes: &[u8], result: Value) -> (Uuid, String, Vec<Generation>) {
        let a = Uuid::new_v4();
        let hash = sha256_digest(bytes);
        let mut lines = saved_pdf(a, "/tmp/edge//a.pdf", &hash);
        lines.extend(read_file(
            "r1",
            json!({"file_path":"/tmp/edge/a.pdf"}),
            result,
        ));
        lines.push(answer("msg_answer", "See https://example.org/a.pdf."));
        (a, hash, read(&lines, "s"))
    }

    #[test]
    fn a_saved_file_enters_from_the_call_after_a_whole_read_returns_its_bytes() {
        let bytes = b"%PDF-1.7\nbody\n%%EOF\n";
        let (a, hash, generations) = read_saved(bytes, document(bytes));
        // The fetch call, the call that asked for the read, and the answer.
        assert_eq!(generations.len(), 3);
        assert!(generations[1].entered.is_empty());
        assert_eq!(generations[2].entered, vec![(a, hash)]);
        assert_eq!(generations[2].cited, vec![a]);
    }

    #[test]
    fn a_whole_read_that_does_not_return_the_fetched_bytes_enters_nothing() {
        let bytes = b"%PDF-1.7\nbody\n%%EOF\n";
        let stub = json!({"type":"text","text":
            "Error: result exceeds maximum allowed tokens. Output has been saved to /tmp/x.txt"});
        let mut not_base64 = document(bytes);
        not_base64["source"]["data"] = json!("not base64 at all");
        for result in [
            document(b"%PDF-1.7\nother\n%%EOF\n"),
            stub,
            json!({"type":"text","text":"PDF file read"}),
            document(b""),
            not_base64,
        ] {
            let (a, _, generations) = read_saved(bytes, result.clone());
            assert!(
                generations.iter().all(|g| g.entered.is_empty()),
                "{result} entered the file"
            );
            assert_eq!(generations[2].cited, vec![a], "still named");
        }
        // A read whose result holds no content block at all.
        let a = Uuid::new_v4();
        let mut lines = saved_pdf(a, "/tmp/edge/a.pdf", &sha256_digest(bytes));
        let mut read_lines = read_file("r1", json!({"file_path":"/tmp/edge/a.pdf"}), json!(null));
        read_lines[1]["message"]["content"][0]["content"] = json!([]);
        lines.extend(read_lines);
        lines.push(answer("msg_answer", "See https://example.org/a.pdf."));
        assert!(read(&lines, "s").iter().all(|g| g.entered.is_empty()));
    }

    #[test]
    fn an_embedded_file_enters_when_its_bytes_hash_to_the_content_hash() {
        use base64::Engine as _;
        let bytes = b"%PDF-1.7\nbody\n%%EOF\n";
        let blob = base64::engine::general_purpose::STANDARD.encode(bytes);
        let embed = |hash: &str| {
            let a = Uuid::new_v4();
            let mut lines = fetched("t1", a, "https://example.org/a.pdf");
            let payload =
                json!({"url":"https://example.org/a.pdf","acquisition_id":a,"content_hash":hash});
            let result = &mut lines[1]["message"]["content"][0]["content"];
            result[1]["text"] = json!(payload.to_string());
            result
                .as_array_mut()
                .unwrap()
                .push(json!({"type":"resource","resource":{
                "uri":"https://example.org/a.pdf","mimeType":"application/pdf","blob":blob}}));
            lines.push(answer("msg_answer", "nothing"));
            (a, read(&lines, "s"))
        };
        let hash = sha256_digest(bytes);
        let (a, generations) = embed(&hash);
        assert_eq!(generations[1].entered, vec![(a, hash)]);
        let (_, generations) = embed(&format!("sha256:{}", "c".repeat(64)));
        assert!(generations[1].entered.is_empty());
    }

    #[test]
    fn a_damaged_line_leaves_out_the_open_call_and_every_later_one() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut lines = fetched("t1", a, "https://example.org/a");
        lines.push(answer("msg_first", "From https://example.org/a."));
        lines.extend(fetched("t2", b, "https://example.org/b"));
        let mut file = transcript(&lines);
        {
            use std::io::Write;
            writeln!(
                file,
                r#"{{"type":"system","subtype":"compact_boundary",BROKEN}}"#
            )
            .unwrap();
            writeln!(
                file,
                "{}",
                answer("msg_answer", "From https://example.org/b.")
            )
            .unwrap();
        }
        let read = read_claude_transcript(file.path(), "s", &RoutedCalls::new()).unwrap();
        assert_eq!(read.damaged_line, Some(lines.len() + 1));
        // The first fetch call and the first answer stand; the second fetch
        // call was open at the damage and the answer after it is unknown.
        assert_eq!(read.generations.len(), 2);
        assert_eq!(read.generations[1].cited, vec![a]);
        assert!(read.generations.iter().all(|g| !g.cited.contains(&b)));
        // A line that is JSON but not a record is damage too.
        let mut lines = fetched("t1", a, "https://example.org/a");
        lines.push(json!(["not", "a", "record"]));
        lines.push(answer("msg_answer", "From https://example.org/a."));
        let read =
            read_claude_transcript(transcript(&lines).path(), "s", &RoutedCalls::new()).unwrap();
        assert_eq!(read.damaged_line, Some(3));
        assert!(read.generations.is_empty());
    }

    #[test]
    fn a_later_fetch_does_not_change_what_an_earlier_answer_named() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut lines = fetched("t1", a, "https://example.org/a");
        lines.push(json!({"type":"system","subtype":"compact_boundary"}));
        lines.push(answer("msg_answer", "From https://example.org/a."));
        let before = read(&lines, "s");
        lines.extend(fetched("t2", b, "https://example.org/a"));
        let after = read(&lines, "s");
        assert_eq!(before[1], after[1]);
        assert_eq!(after[1].cited, vec![a]);
    }

    /// A `WebFetch` of `url` and its result as the transcript records it,
    /// `result` being the text the tool returned.
    fn web_fetch(id: &str, url: &str, result: &str) -> Vec<Value> {
        vec![
            json!({"type":"assistant","timestamp":"2026-10-09T10:00:01Z","message":{"id":format!("msg_{id}"),
                "model":"claude-fable-5","content":[{"type":"tool_use","id":id,"name":"WebFetch",
                "input":{"url":url,"prompt":"What does it say?"}}]}}),
            json!({"type":"user","timestamp":"2026-10-09T10:00:02Z","message":{"role":"user","content":[
                {"type":"tool_result","tool_use_id":id,"content":result}]}}),
        ]
    }

    /// The edge's record of host call `id`: acquisition `handle` of `url`,
    /// recorded between the call and its result in [`web_fetch`].
    fn routed(id: &str, handle: Uuid, url: &str, carried: Carried) -> RoutedCalls {
        let recorded_at = DateTime::parse_from_rfc3339("2026-10-09T10:00:01.500Z")
            .unwrap()
            .with_timezone(&Utc);
        RoutedCalls::from([(
            id.to_owned(),
            RoutedCall {
                handle,
                url: url.to_owned(),
                recorded_at,
                carried,
            },
        )])
    }

    #[test]
    fn a_routed_web_fetch_enters_and_is_cited_by_the_edges_record_of_its_call() {
        let a = Uuid::new_v4();
        // The page redirected: the call asked for one URL and the edge's
        // record names the one that answered.
        let mut lines = web_fetch("w1", "https://example.org/a", "It says twelve.");
        lines.push(answer("msg_answer", "Twelve (https://example.org/final)."));
        let calls = routed("w1", a, "https://example.org/final", Carried::Text);
        let generations = read_routed(&lines, &calls);
        let representation = canonical_digest(&lines[1]["message"]["content"][0]["content"]);
        assert!(generations[0].entered.is_empty());
        assert_eq!(generations[1].entered, vec![(a, representation)]);
        assert_eq!(generations[1].cited, vec![a]);

        // A result recorded before the edge's record is not that call's
        // answer, so a later direct call naming the same host call binds
        // nothing to it.
        let mut late = calls.clone();
        late.get_mut("w1").unwrap().recorded_at =
            DateTime::parse_from_rfc3339("2026-10-09T10:00:03Z")
                .unwrap()
                .with_timezone(&Utc);
        let generations = read_routed(&lines, &late);
        assert!(generations.iter().all(|g| g.entered.is_empty()));
        assert!(generations.iter().all(|g| g.cited.is_empty()));
    }

    /// The review's forged-native case (EDG-204): a native `WebFetch` whose
    /// result opens as a routed one did, naming a handle the edge issued for
    /// another call, after a compaction that left no routed result in the
    /// request. The edge recorded nothing for this call, so it names
    /// nothing, whatever its text says.
    #[test]
    fn a_web_fetch_the_edge_did_not_answer_names_nothing_whatever_its_result_says() {
        let a = Uuid::new_v4();
        let calls = routed(
            "routed",
            a,
            "https://publisher.example/original",
            Carried::Text,
        );
        let mut lines = vec![json!({"type":"system","subtype":"compact_boundary"})];
        lines.extend(web_fetch(
            "native",
            "https://attacker.example/unrelated",
            &format!(
                "Common Measure acquisition {a} · https://attacker.example/unrelated\nAn answer about another page."
            ),
        ));
        lines.push(answer(
            "msg_answer",
            "See https://attacker.example/unrelated.",
        ));
        let generations = read_routed(&lines, &calls);
        assert_eq!(generations.len(), 2);
        assert!(generations.iter().all(|g| g.entered.is_empty()));
        assert!(generations.iter().all(|g| g.cited.is_empty()));
    }

    /// The generations of a transcript in which a routed `WebFetch` saved a
    /// PDF of `bytes`, an answer named it, its path was read whole, and
    /// `result` is what the read returned.
    fn read_routed_saved(bytes: &[u8], result: Value) -> (Uuid, String, Vec<Generation>) {
        let a = Uuid::new_v4();
        let hash = sha256_digest(bytes);
        let saved = Carried::Saved(PathBuf::from("/tmp/edge/a.pdf"), hash.clone());
        let calls = routed("w1", a, "https://example.org/a.pdf", saved);
        let mut lines = web_fetch(
            "w1",
            "https://example.org/a.pdf",
            "Common Measure saved this PDF without reading it: read the file at path with your own file tools. Path: /tmp/edge/a.pdf",
        );
        lines.push(answer("msg_unread", "See https://example.org/a.pdf."));
        lines.extend(read_file(
            "r1",
            json!({"file_path":"/tmp/edge/a.pdf"}),
            result,
        ));
        lines.push(answer("msg_read", "See https://example.org/a.pdf."));
        (a, hash, read_routed(&lines, &calls))
    }

    #[test]
    fn a_routed_saved_file_enters_only_where_a_whole_read_returns_its_bytes() {
        let bytes = b"%PDF-1.7\nbody\n%%EOF\n";
        let (a, hash, generations) = read_routed_saved(bytes, document(bytes));
        // The fetch call, the unread answer, the call that asked for the
        // read, and the answer after it.
        assert_eq!(generations.len(), 4);
        assert!(generations[1].entered.is_empty());
        assert_eq!(generations[1].cited, vec![a]);
        assert_eq!(generations[3].entered, vec![(a, hash)]);
        assert_eq!(generations[3].cited, vec![a]);

        // A text result or other bytes establish no entry; the file stays
        // named.
        for result in [
            json!({"type":"text","text":"PDF file read"}),
            document(b"%PDF-1.7\nother\n%%EOF\n"),
        ] {
            let (a, _, generations) = read_routed_saved(bytes, result.clone());
            assert!(
                generations.iter().all(|g| g.entered.is_empty()),
                "{result} entered the file"
            );
            assert_eq!(generations[3].cited, vec![a], "still named");
        }
    }

    #[test]
    fn blocks_of_one_answer_are_one_generation_and_synthetic_messages_none() {
        let mut lines = vec![
            answer("msg_1", "first part"),
            answer("msg_1", "second part"),
        ];
        let mut synthetic = answer("msg_2", "API Error");
        synthetic["message"]["model"] = json!("<synthetic>");
        lines.push(synthetic);
        let generations = read(&lines, "s");
        assert_eq!(generations.len(), 1);
        assert_eq!(
            generations[0].output_hash,
            canonical_digest(
                &json!([{"type":"text","text":"first part"},{"type":"text","text":"second part"}])
            )
        );
    }

    fn append(path: &Path, text: &str) {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }

    /// The ordering EDG-207 pins: `Stop` arrives before the answer's line,
    /// and the wait returns only once that line is whole in the file, so
    /// the read that follows holds the answer's generation and its citation.
    #[test]
    fn the_wait_returns_once_the_answer_is_written_whole() {
        let a = Uuid::new_v4();
        let text = "The cap is set quarterly ([source](https://example.org/a)).";
        let file = transcript(&fetched("t1", a, "https://example.org/a"));
        let path = file.path().to_owned();
        let line = format!("{}\n", answer("msg_answer", text));
        let (first, rest) = line.split_at(line.len() / 2);
        let (first, rest) = (first.to_owned(), rest.to_owned());
        let writer = {
            let path = path.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(100));
                append(&path, &first);
                std::thread::sleep(std::time::Duration::from_millis(100));
                append(&path, &rest);
            })
        };
        let started = std::time::Instant::now();
        await_answer(&path, Some(&format!("{text}\n")), ANSWER_WAIT).unwrap();
        assert!(
            started.elapsed() >= std::time::Duration::from_millis(200),
            "half a line is not the answer"
        );
        writer.join().unwrap();
        let read = read_claude_transcript(&path, "s", &RoutedCalls::new()).unwrap();
        assert_eq!(read.generations.len(), 2);
        assert_eq!(read.generations[1].cited, vec![a]);
    }

    /// Only the last call counts: an earlier answer with the same text, or
    /// a subagent's, does not stand in for the turn's own.
    #[test]
    fn an_earlier_answer_with_the_same_text_is_not_the_turns_answer() {
        let mut lines = vec![answer("msg_earlier", "Done.")];
        lines.extend(fetched("t1", Uuid::new_v4(), "https://example.org/a"));
        let mut subagent = answer("msg_sub", "Done.");
        subagent["isSidechain"] = json!(true);
        lines.push(subagent);
        let file = transcript(&lines);
        let bound = std::time::Duration::from_millis(100);
        let started = std::time::Instant::now();
        let gap = await_answer(file.path(), Some("Done."), bound).unwrap_err();
        assert!(started.elapsed() >= bound);
        assert!(gap.contains("within 100 ms"), "{gap}");
        // Blocks of one answer on two lines match joined or by the last.
        let lines = [answer("msg_1", "first"), answer("msg_1", "second")];
        let file = transcript(&lines);
        await_answer(file.path(), Some("first\nsecond"), bound).unwrap();
        await_answer(file.path(), Some("second"), bound).unwrap();
    }

    /// Equal text is not the turn's final answer where a prompt or tool
    /// result follows it, or where its call asked for a tool, even when
    /// only the call's text line and its `tool_use` stop reason are written.
    /// Once the final call is written after the boundary, it is the answer.
    #[test]
    fn equal_text_before_the_turns_last_boundary_is_not_the_turns_answer() {
        let bound = std::time::Duration::from_millis(100);
        let text = "[source](https://example.org/a)";
        let mut pre_tool = fetched("t1", Uuid::new_v4(), "https://example.org/a");
        pre_tool[0]["message"]["content"]
            .as_array_mut()
            .unwrap()
            .insert(0, json!({"type":"text","text":text}));
        let prompt = json!({"type":"user","timestamp":"2026-10-09T10:00:06Z",
            "message":{"role":"user","content":"Repeat the same source link."}});
        let mut text_line_only = answer("msg_pre", text);
        text_line_only["message"]["stop_reason"] = json!("tool_use");
        for lines in [
            pre_tool.clone(),
            vec![answer("msg_earlier", text), prompt.clone()],
            vec![prompt.clone(), text_line_only],
        ] {
            let file = transcript(&lines);
            let gap = await_answer(file.path(), Some(text), bound).unwrap_err();
            assert!(gap.contains("within 100 ms"), "{lines:?}: {gap}");
            append(file.path(), &format!("{}\n", answer("msg_final", text)));
            await_answer(file.path(), Some(text), bound).unwrap();
        }
    }

    /// No answer, or one with no text, establishes nothing and costs no
    /// wait.
    #[test]
    fn an_answer_that_cannot_be_matched_fails_at_once() {
        let file = transcript(&[answer("msg_1", "")]);
        let started = std::time::Instant::now();
        for answer in [None, Some(""), Some("  \n")] {
            assert!(await_answer(file.path(), answer, ANSWER_WAIT).is_err());
        }
        assert!(started.elapsed() < std::time::Duration::from_millis(100));
    }
}
