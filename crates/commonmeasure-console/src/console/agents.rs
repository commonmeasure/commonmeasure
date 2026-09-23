//! The Agents screen at `/app/agents` renders `/api/agents`: the host
//! sessions on this edge, what each can be seen doing, which policy it
//! loaded and what needs attention.
//!
//! The store supplies host sessions joined on the `host_process` pair
//! (`Store::host_sessions`). This module resolves what needs something
//! outside the index: liveness from an injected probe, the comparison
//! against the policy the edge holds now from the Policy screen's
//! projection, delivery problems from the relay's egress report, and
//! coverage from the path table in `docs/contracts/host-integration.md` §6.
//!
//! The states are the contract's (`docs/contracts/session-evidence.md`
//! §Host process). A process is reported running only when the probe found
//! it in the process table; the absence of `session_ended` is never read as
//! running, and a console with no probe says liveness is unavailable.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use maud::{Markup, html};
use serde_json::{Value, json};

use super::app::{ReadFrom, Section, fact, shell, str_of, topbar, u, when};
use super::form::encode_component;

/// Answers whether the process `(pid, started_at)` is in the process table
/// now: `Some(true)` present, `Some(false)` absent, `None` when the table
/// could not be read. `started_at` is the record's RFC 3339 string to the
/// second. Injected by the caller, which owns process-table reading; the
/// console never reads the table itself.
pub type LivenessProbe = Arc<dyn Fn(u32, &str) -> Option<bool> + Send + Sync>;

/// How long a running host process may hold a log that records no work
/// before the screen lists it for attention.
const IDLE_ATTENTION_AFTER_MINUTES: i64 = 60;

/// A session's items stay on the screen's attention list while its host
/// process is present or its last record is this recent. An older session's
/// items remain in its own detail.
const ATTENTION_WINDOW_HOURS: i64 = 24;

/// The `policy_sync` outcomes that leave the desired revision in force
/// (`docs/contracts/policy-envelope.md`, the outcome table). Every other
/// outcome is listed for attention.
const SYNC_OUTCOMES_IN_FORCE: [&str; 3] = ["accepted", "reapplied", "already_applied"];

/// The four paths a host can reach Common Measure by, in display order.
const PATHS: [&str; 4] = ["mediated", "observed", "reconstructed", "hosted"];

/// One host word's paths: `Ok(state)` is the verification state §6 of the
/// host integration contract gives the path; `Err(reason)` says the host has
/// no such path. Order follows [`PATHS`].
type HostPaths = [Result<&'static str, &'static str>; 4];

const NO_HOOKS: &str = "the host runs no hooks";
const NO_IMPORTER: &str = "no importer reads this host's transcripts";
const NOT_HOSTED: &str = "the host reaches the edge over stdio, not HTTP";
const HOSTED_ONLY: &str = "a hosted host reaches the edge over HTTP only";
const BROWSER_ONLY: &str = "a browser surface is observed by the extension only";

/// The path table, from `docs/contracts/host-integration.md` §6. A host
/// word the table does not list has no entry, and the screen says so.
fn paths_of(host: &str) -> Option<HostPaths> {
    Some(match host {
        "claude-code" => [
            Ok("live-verified"),
            Ok("fixture-tested"),
            Ok("fixture-tested"),
            Err(NOT_HOSTED),
        ],
        "codex" => [
            Ok("fixture-tested"),
            Err(NO_HOOKS),
            Err(NO_IMPORTER),
            Err(NOT_HOSTED),
        ],
        "pi" => [
            Ok("fixture-tested"),
            Ok("fixture-tested"),
            Err(NO_IMPORTER),
            Err(NOT_HOSTED),
        ],
        "claude-desktop" | "vscode" => [
            Ok("planned"),
            Err(NO_HOOKS),
            Err(NO_IMPORTER),
            Err(NOT_HOSTED),
        ],
        "cursor" | "copilot-cli" => [
            Ok("planned"),
            Ok("spec-verified"),
            Err(NO_IMPORTER),
            Err(NOT_HOSTED),
        ],
        "chatgpt-web" | "bing-copilot-search" => [
            Err(BROWSER_ONLY),
            Ok("fixture-tested"),
            Err(BROWSER_ONLY),
            Err(BROWSER_ONLY),
        ],
        "google-ai-overview" => [
            Err(BROWSER_ONLY),
            Ok("planned"),
            Err(BROWSER_ONLY),
            Err(BROWSER_ONLY),
        ],
        "m365-copilot" => [
            Err(HOSTED_ONLY),
            Err(HOSTED_ONLY),
            Err(HOSTED_ONLY),
            Ok("live-verified"),
        ],
        "claude-connector" | "chatgpt" | "copilot-cloud-agent" => [
            Err(HOSTED_ONLY),
            Err(HOSTED_ONLY),
            Err(HOSTED_ONLY),
            Ok("planned"),
        ],
        _ => return None,
    })
}

/// The projection `/api/agents` serves and the screen renders.
///
/// `host_sessions` is `Store::host_sessions`; `policy` is the Policy
/// screen's projection, unfiltered; `egress` is the relay's egress report.
pub fn projection(
    host_sessions: &Value,
    policy: &Value,
    egress: &Value,
    probe: Option<&LivenessProbe>,
    now: DateTime<Utc>,
) -> Value {
    let held = held_now(policy);
    let mut attention = delivery_attention(egress);
    let mut sessions = Vec::new();
    for session in host_sessions.as_array().into_iter().flatten() {
        let mut session = session.clone();
        session["liveness"] = liveness_of(&session, probe);
        session["policy"]["comparison"] = policy_comparison(&session, &held);
        let items = session_attention(&session, now);
        if is_current(&session, now) {
            attention.extend(items.iter().cloned());
        }
        session["attention"] = Value::Array(items);
        sessions.push(session);
    }
    json!({
        "sessions": sessions,
        "coverage": coverage(host_sessions),
        "attention": attention,
        "attention_window_hours": ATTENTION_WINDOW_HOURS,
        "liveness_probe": if probe.is_some() { "wired" } else { "unavailable" },
        "policy_held_now": match &held {
            Ok(identities) => json!({"identities": identities}),
            Err(reason) => json!({"unavailable": reason}),
        },
    })
}

/// Whether a session's items belong on the screen's list: its host process
/// is present, or it recorded something inside the window.
fn is_current(session: &Value, now: DateTime<Utc>) -> bool {
    matches!(
        session["liveness"]["state"].as_str(),
        Some("running" | "seen_working")
    ) || session["last"]
        .as_str()
        .and_then(|stamp| DateTime::parse_from_rfc3339(stamp).ok())
        .is_some_and(|last| {
            now - last.with_timezone(&Utc) <= Duration::hours(ATTENTION_WINDOW_HOURS)
        })
}

/// The policy identities the edge holds now: one per scope and the
/// top-level policy, as the Policy screen's projection resolved them.
fn held_now(policy: &Value) -> Result<BTreeSet<String>, String> {
    if let Some(error) = policy["error"].as_str() {
        return Err(format!("the policy could not be read: {error}"));
    }
    if policy["declared"] != Value::Bool(true) {
        return Err(format!(
            "no policy is declared at {}",
            str_of(policy.get("source"), "policy.json")
        ));
    }
    Ok(policy["modes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|mode| mode["policy_identity"].as_str())
        .map(str::to_owned)
        .collect())
}

/// Each member log's latest policy identity against the identities the edge
/// holds now.
fn policy_comparison(session: &Value, held: &Result<BTreeSet<String>, String>) -> Value {
    let logs: Vec<Value> = session["logs"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|log| {
            let id = &log["session_id"];
            let Some(identity) = log["latest_policy_identity"].as_str() else {
                return json!({
                    "session_id": id,
                    "state": "not_recorded",
                    "line": "no policy identity recorded",
                });
            };
            match held {
                Err(reason) => json!({
                    "session_id": id, "identity": identity,
                    "state": "unavailable",
                    "line": format!("comparison unavailable: {reason}"),
                }),
                Ok(identities) if identities.contains(identity) => json!({
                    "session_id": id, "identity": identity,
                    "state": "current",
                    "line": "the policy the edge holds now",
                }),
                Ok(_) => json!({
                    "session_id": id, "identity": identity,
                    "state": "superseded",
                    "line": "newer policy, not loaded by this session",
                }),
            }
        })
        .collect();
    json!({"logs": logs})
}

/// The host process's state, from the probe and the `session_ended` record.
fn liveness_of(session: &Value, probe: Option<&LivenessProbe>) -> Value {
    let worked = session["worked"] == Value::Bool(true);
    let state = |state: &str, label: String, basis: String| json!({"state": state, "label": label, "basis": basis, "worked": worked});
    if session["ended"].is_object() {
        let label = match session["ended"]["reason"].as_str() {
            Some(reason) => format!("ended ({reason})"),
            None => "ended".to_owned(),
        };
        return state(
            "ended",
            label,
            "the host reported the session ending (session_ended)".to_owned(),
        );
    }
    let unavailable =
        |reason: String| state("unavailable", "liveness unavailable".to_owned(), reason);
    let process = &session["host_process"];
    let (Some(pid), Some(started_at)) = (process["pid"].as_u64(), process["started_at"].as_str())
    else {
        return unavailable(
            str_of(session["join"].get("detail"), "no host_process record").to_owned(),
        );
    };
    let Some(probe) = probe else {
        return unavailable("no liveness probe is wired into this console".to_owned());
    };
    let Ok(pid) = u32::try_from(pid) else {
        return unavailable(format!("the recorded pid {pid} is not a process id"));
    };
    match probe(pid, started_at) {
        None => unavailable("the process table could not be read".to_owned()),
        Some(true) if worked => state(
            "seen_working",
            "seen working".to_owned(),
            "the host process is in the process table and the logs record a turn boundary or a crossing".to_owned(),
        ),
        Some(true) => state(
            "running",
            "running".to_owned(),
            "the host process is in the process table; the logs record no turn boundary and no crossing".to_owned(),
        ),
        Some(false) => state(
            "gone_without_session_end",
            "gone without a session end".to_owned(),
            "the host process is not in the process table and no session_ended record exists".to_owned(),
        ),
    }
}

/// Per host word: which paths the evidence on this edge shows, and which the
/// host has no path for.
fn coverage(host_sessions: &Value) -> Value {
    // host word -> path -> the evidence that shows it
    let mut shown: BTreeMap<String, BTreeMap<&'static str, String>> = BTreeMap::new();
    let mut sessions: BTreeMap<String, u64> = BTreeMap::new();
    for session in host_sessions.as_array().into_iter().flatten() {
        if let Some(host) = session["host"].as_str() {
            *sessions.entry(host.to_owned()).or_default() += 1;
        }
        for log in session["logs"].as_array().into_iter().flatten() {
            let Some(host) = log["host"].as_str() else {
                continue;
            };
            let id = str_of(log.get("session_id"), "");
            let events = &log["events"];
            let has = |event: &str| events[event].as_u64().unwrap_or(0) > 0;
            let paths = shown.entry(host.to_owned()).or_default();
            let mut show = |path: &'static str, evidence: String| {
                paths.entry(path).or_insert(evidence);
            };
            if id.starts_with("hosted-") || has("hosted_scope") {
                show("hosted", format!("hosted session log {id}"));
            } else if id.starts_with("local-") {
                show("mediated", format!("MCP server log {id}"));
            } else if has("crossing_mediated")
                || has("crossing_refused")
                || has("client_identified")
            {
                show("mediated", format!("mediated records in {id}"));
            }
            if has("turn_started") || has("crossing_observed") {
                show("observed", format!("hook records in {id}"));
            }
            if has("crossing_reconstructed") {
                show("reconstructed", format!("imported records in {id}"));
            }
        }
    }
    Value::Array(
        shown
            .into_iter()
            .map(|(host, evidence)| {
                let table = paths_of(&host);
                let paths: Vec<Value> = PATHS
                    .iter()
                    .enumerate()
                    .map(|(index, path)| {
                        let (state, no_path) = match &table {
                            Some(table) => match table[index] {
                                Ok(state) => (Some(state), None),
                                Err(reason) => (None, Some(reason)),
                            },
                            None => (None, None),
                        };
                        json!({
                            "path": path,
                            "shown": evidence.contains_key(path),
                            "shown_by": evidence.get(path),
                            "verification_state": state,
                            "no_path": no_path,
                        })
                    })
                    .collect();
                json!({
                    "host": host,
                    "host_sessions": sessions.get(&host).copied().unwrap_or(0),
                    "in_path_table": table.is_some(),
                    "paths": paths,
                })
            })
            .collect(),
    )
}

fn item(
    kind: &str,
    session_id: Option<&str>,
    summary: String,
    evidence: String,
    next: &str,
) -> Value {
    json!({
        "kind": kind,
        "session_id": session_id,
        "summary": summary,
        "evidence": evidence,
        "next_action": next,
        "link": match session_id {
            Some(id) => format!("/app/record?session={}", encode_component(id)),
            None => "/app".to_owned(),
        },
    })
}

/// Delivery problems, from the relay's egress report. They belong to the
/// edge and to no session, so they link to the Overview's hub panel.
fn delivery_attention(egress: &Value) -> Vec<Value> {
    let mut items = Vec::new();
    if let Some(reason) = egress["unavailable"].as_str() {
        items.push(item(
            "delivery",
            None,
            "Relay state could not be read".to_owned(),
            format!("egress report: {reason}"),
            "Repair relay.json or the relay state file; nothing is sent until it parses.",
        ));
    }
    if let Some(dead) = egress["dead"].as_u64().filter(|dead| *dead > 0) {
        items.push(item(
            "delivery",
            None,
            format!("{dead} dead relay batches"),
            match egress["last_error"].as_str() {
                Some(error) => format!("egress report; last delivery error: {error}"),
                None => "egress report".to_owned(),
            },
            "Fix the cause, then run `commonmeasure relay requeue`.",
        ));
    } else if let Some(error) = egress["last_error"].as_str() {
        items.push(item(
            "delivery",
            None,
            "The last delivery attempt failed".to_owned(),
            format!("egress report: {error}"),
            "The relay retries on its schedule; check the receiver if the error repeats.",
        ));
    }
    if let Some(held) = egress["held"].as_u64().filter(|held| *held > 0) {
        items.push(item(
            "delivery",
            None,
            format!("{held} relay batches held"),
            format!(
                "egress report; hold reason: {}",
                str_of(egress.get("hold_reason"), "not recorded")
            ),
            "Read the hold reason; a policy hold lifts when the policy clears the events.",
        ));
    }
    items
}

/// What one host session's records say needs attention.
fn session_attention(session: &Value, now: DateTime<Utc>) -> Vec<Value> {
    let mut items = Vec::new();
    let primary = str_of(session.get("id"), "");
    for sync in session["policy"]["syncs"].as_array().into_iter().flatten() {
        let id = sync["session_id"].as_str();
        let outcome = str_of(sync.get("outcome"), "unknown");
        if !SYNC_OUTCOMES_IN_FORCE.contains(&outcome) {
            items.push(item(
                "policy_sync",
                id,
                format!("Policy refresh outcome: {outcome}"),
                match sync["reason"].as_str() {
                    Some(reason) => format!("policy_sync record: {reason}"),
                    None => "policy_sync record".to_owned(),
                },
                "Run `commonmeasure policy sync` and read its outcome.",
            ));
        }
        if let Some(since) = sync["stale_since"].as_str() {
            items.push(item(
                "policy_stale",
                id,
                format!("Managed policy stale since {since}"),
                format!(
                    "policy_sync record, revision {} in force",
                    sync["applied"]["revision"]
                ),
                "Run `commonmeasure policy sync`; the expired revision stays in force until a refresh succeeds.",
            ));
        }
    }
    let identity = &session["edge_identity"];
    if identity["standing"] == "revoked" {
        items.push(item(
            "edge_identity",
            Some(primary),
            "The session ran under a revoked key".to_owned(),
            format!(
                "edge_identity record: key {} revoked at {}",
                str_of(identity.get("key_id"), "unknown"),
                str_of(identity.get("revoked_at"), "unknown")
            ),
            "Enrol again with `commonmeasure enrol`; requests signed with a revoked key verify nowhere.",
        ));
    }
    if let Some(reason) = identity["unlisted"].as_str() {
        items.push(item(
            "edge_identity",
            Some(primary),
            "The hub's key directory does not list this edge's key".to_owned(),
            format!("edge_identity record: {reason}"),
            "Run `commonmeasure status` and renew the directory listing.",
        ));
    }
    let divergent = session["policy"]["divergent_members"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    if !divergent.is_empty() {
        items.push(item(
            "policy_divergence",
            Some(primary),
            "Logs of one host session loaded different policies".to_owned(),
            format!(
                "policy identities and policy_sync revisions in {}",
                divergent
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            "Restart the host session so its hooks and its MCP server load one policy.",
        ));
    }
    let refusals = session["unresolved_refusals"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    if let Some(first) = refusals.first() {
        items.push(item(
            "unresolved_refusal",
            first["session_id"].as_str(),
            match refusals.len() {
                1 => "1 refused crossing with no later mediated crossing of the same host".to_owned(),
                many => format!(
                    "{many} refused crossings with no later mediated crossing of the same host"
                ),
            },
            format!(
                "crossing_refused for {}: {}",
                str_of(first.get("host_name"), "unknown host"),
                str_of(first.get("refusal"), "no reason recorded")
            ),
            "Read the refusal in the record; change the source policy only if the source should be admitted.",
        ));
    }
    if u(session.get("evidence_gaps")) > 0 {
        items.push(item(
            "evidence_gap",
            Some(primary),
            format!("{} evidence gaps", u(session.get("evidence_gaps"))),
            "evidence_gap records: windows the log could not record".to_owned(),
            "Check the sessions directory is writable and has space.",
        ));
    }
    let first = session["first"]
        .as_str()
        .and_then(|stamp| DateTime::parse_from_rfc3339(stamp).ok())
        .map(|stamp| stamp.with_timezone(&Utc));
    if session["liveness"]["state"] == "running"
        && let Some(first) = first
        && now - first > Duration::minutes(IDLE_ATTENTION_AFTER_MINUTES)
    {
        items.push(item(
            "idle_integration",
            Some(primary),
            format!(
                "Running for {} and never seen working",
                age(now - first)
            ),
            format!(
                "host process {} is in the process table; the log holds start records only",
                session["host_process"]["pid"]
            ),
            "Close the host's unused server, or run `commonmeasure doctor` for the host if it should be sending calls.",
        ));
    }
    items
}

fn age(span: Duration) -> String {
    match (span.num_days(), span.num_hours()) {
        (days, _) if days >= 2 => format!("{days} days"),
        (_, hours) if hours >= 1 => format!("{hours} h"),
        _ => format!("{} min", span.num_minutes()),
    }
}

// ---- rendering ----

/// The Agents screen. `selected` is one host session rendered into the pane
/// by the server, named by any of its member logs.
pub fn page(agents: &Value, selected: Option<&str>, read: ReadFrom<'_>) -> String {
    shell(Some(Section::Agents), screen(agents, selected, read)).into_string()
}

/// The detail pane for one host session, loaded by htmx.
pub fn fragment(agents: &Value, id: &str) -> String {
    detail(agents, id).into_string()
}

fn find<'a>(agents: &'a Value, id: &str) -> Option<&'a Value> {
    agents["sessions"].as_array()?.iter().find(|session| {
        session["logs"]
            .as_array()
            .is_some_and(|logs| logs.iter().any(|log| log["session_id"] == id))
    })
}

/// One working directory's host sessions in the rail.
type Directory<'a> = (String, Vec<&'a Value>);

fn screen(agents: &Value, selected: Option<&str>, read: ReadFrom<'_>) -> Markup {
    let empty = Vec::new();
    let sessions = agents["sessions"].as_array().unwrap_or(&empty);
    // Host word, then working directory, each in order of most recent
    // activity: the projection is already sorted that way.
    let mut hosts: Vec<(String, Vec<Directory<'_>>)> = Vec::new();
    for session in sessions {
        let host = str_of(session.get("host"), "host not recorded").to_owned();
        let cwd = match session["cwds"].as_array().map(Vec::as_slice) {
            Some([only]) => only.as_str().unwrap_or("").to_owned(),
            Some([]) | None => "no working directory recorded".to_owned(),
            Some(many) => format!("{} working directories", many.len()),
        };
        let position = match hosts.iter().position(|(name, _)| *name == host) {
            Some(position) => position,
            None => {
                hosts.push((host, Vec::new()));
                hosts.len() - 1
            }
        };
        let directories = &mut hosts[position].1;
        match directories.iter().position(|(name, _)| *name == cwd) {
            Some(position) => directories[position].1.push(session),
            None => directories.push((cwd, vec![session])),
        }
    }
    let selected_session = selected.and_then(|id| find(agents, id));
    let selected_id = selected_session.and_then(|session| session["id"].as_str());
    html! {
        (topbar("Agents", html! {
            "Read from " span class="mono" { (read.sessions) } " at " (read.at) "."
            @if agents["liveness_probe"] != "wired" { " Liveness unavailable: no probe is wired." }
        }))
        section class="screen" {
            div class="cards stack" {
                (attention_card(&agents["attention"]))
                (coverage_card(&agents["coverage"]))
            }
        }
        section class="screen record" {
            div class="md" {
                aside class="md-rail" aria-label="Host sessions" {
                    @if sessions.is_empty() {
                        p class="muted" { "No sessions recorded." }
                    }
                    @for (host, directories) in &hosts {
                        h2 class="md-group" { (host) }
                        @for (cwd, members) in directories {
                            p class="note mono" { (cwd) }
                            @for session in members { (rail_row(session, selected_id)) }
                        }
                    }
                }
                section class="md-detail" id="detail" aria-label="Host session" {
                    @match selected {
                        Some(id) => (detail(agents, id)),
                        None => p class="muted pad" { "No host session selected." },
                    }
                }
            }
        }
    }
}

fn attention_card(attention: &Value) -> Markup {
    let empty = Vec::new();
    let items = attention.as_array().unwrap_or(&empty);
    html! {
        div class="card" {
            h2 { "Attention" span class="muted-inline" { (items.len()) } }
            p class="callout" {
                "Host sessions with a running host process or a record in the last " (ATTENTION_WINDOW_HOURS) " hours, and the edge's delivery state. An older session's items are in its detail."
            }
            @if items.is_empty() {
                p class="muted" { "Nothing recorded needs attention." }
            } @else {
                table class="rules" {
                    thead { tr { th { "Issue" } th { "Evidence" } th { "Next action" } th { "Record" } } }
                    tbody {
                        @for item in items { (attention_row(item)) }
                    }
                }
            }
        }
    }
}

fn attention_row(item: &Value) -> Markup {
    html! {
        tr {
            td { (str_of(item.get("summary"), "")) }
            td { (str_of(item.get("evidence"), "")) }
            td { (str_of(item.get("next_action"), "")) }
            td class="mono" {
                a href=(str_of(item.get("link"), "/app")) {
                    (item["session_id"].as_str().unwrap_or("Overview"))
                }
            }
        }
    }
}

fn coverage_card(coverage: &Value) -> Markup {
    let empty = Vec::new();
    let hosts = coverage.as_array().unwrap_or(&empty);
    html! {
        div class="card" {
            h2 { "Coverage by host" }
            @if hosts.is_empty() {
                p class="muted" { "No host has recorded anything." }
            } @else {
                table class="rules" {
                    thead { tr {
                        th { "Host" }
                        @for path in PATHS { th { (path) } }
                    } }
                    tbody {
                        @for host in hosts {
                            tr {
                                td class="mono" { (str_of(host.get("host"), "")) }
                                @for path in host["paths"].as_array().into_iter().flatten() {
                                    td { (coverage_cell(path, host["in_path_table"] == true)) }
                                }
                            }
                        }
                    }
                }
                p class="callout" {
                    "A mediated path covers the host's calls to Common Measure's tools only; the host's other tools are seen on the observed path or not at all."
                }
            }
        }
    }
}

fn coverage_cell(path: &Value, in_table: bool) -> Markup {
    let shown = path["shown"] == Value::Bool(true);
    html! {
        @if shown {
            span class="badge b-ok" { "seen" }
            @if let Some(state) = path["verification_state"].as_str() { " " span class="muted-inline" { (state) } }
        } @else if let Some(reason) = path["no_path"].as_str() {
            span class="muted-inline" { "unavailable: " (reason) }
        } @else if !in_table {
            span class="muted-inline" { "host not in the path table" }
        } @else {
            span class="muted-inline" { "not seen on this edge" }
        }
    }
}

fn rail_row(session: &Value, selected: Option<&str>) -> Markup {
    let id = str_of(session.get("id"), "unknown");
    let encoded = encode_component(id);
    let page = format!("/app/agents?agent={encoded}");
    let attention = session["attention"].as_array().map_or(0, Vec::len);
    let name = match (
        session["client"]["name"].as_str(),
        session["client"]["version"].as_str(),
    ) {
        (Some(name), Some(version)) => format!("{name} {version}"),
        (Some(name), None) => name.to_owned(),
        _ => str_of(session["host_process"].get("command"), "").to_owned(),
    };
    html! {
        a class="srow" href=(page)
            hx-get=(format!("/app/fragments/agent/{encoded}"))
            hx-target="#detail" hx-swap="innerHTML" hx-push-url=(page)
            aria-current=[(selected == Some(id)).then_some("true")]
        {
            span class="srow-top" {
                span class="mono" { (id) }
                small { (name) }
            }
            span class="srow-meta" {
                (str_of(session["liveness"].get("label"), "liveness unavailable"))
                @if session["worked"] != Value::Bool(true) { " · never seen working" }
                " · " (when(session.get("last")))
                @if attention > 0 { " · " (attention) " to check" }
            }
        }
    }
}

fn detail(agents: &Value, id: &str) -> Markup {
    let Some(session) = find(agents, id) else {
        return html! {
            h2 class="detail-head" tabindex="-1" autofocus { "Host session " span class="mono id" { (id) } }
            p class="muted" { "No log " (id) " in the index." }
        };
    };
    let refused = u(session.get("refused"));
    let process = &session["host_process"];
    html! {
        h2 class="detail-head" tabindex="-1" autofocus {
            "Host session " span class="mono id" { (str_of(session.get("id"), id)) }
        }
        div class="facts" {
            (fact("state", str_of(session["liveness"].get("label"), "liveness unavailable"), false))
            (fact("work", if session["worked"] == true { "turn or crossing recorded" } else { "configured, never seen working" }, false))
            (fact("turn boundaries", &u(session.get("turns")).to_string(), false))
            (fact("mediated", &u(session.get("mediated")).to_string(), false))
            (fact("observed", &u(session.get("observed")).to_string(), false))
            (fact("reconstructed", &u(session.get("reconstructed")).to_string(), false))
            (fact("refused", &refused.to_string(), refused > 0))
        }

        h3 class="xh" { "Timeline" }
        dl class="cx-fields" {
            (field("Host", str_of(session.get("host"), "not recorded")))
            @if let Some(name) = session["client"]["name"].as_str() {
                (field("Client", &format!("{name} {}", str_of(session["client"].get("version"), ""))))
            }
            @if process.is_object() {
                (field("Host process", &format!(
                    "{} pid {} started {}",
                    str_of(process.get("command"), "unknown"),
                    process["pid"],
                    str_of(process.get("started_at"), "unknown"),
                )))
            }
            (field("Liveness basis", str_of(session["liveness"].get("basis"), "")))
            (field("Join", &format!(
                "{}: {}",
                str_of(session["join"].get("basis"), ""),
                str_of(session["join"].get("detail"), ""),
            )))
            (field("First record", &when(session.get("first"))))
            (field("Last record", &when(session.get("last"))))
            @if session["ended"].is_object() {
                (field("Session ended", &format!(
                    "{} ({})",
                    when(session["ended"].get("timestamp")),
                    str_of(session["ended"].get("reason"), "no reason given"),
                )))
            }
            @for cwd in session["cwds"].as_array().into_iter().flatten() {
                (field("Working directory", cwd.as_str().unwrap_or("")))
            }
        }

        h3 class="xh" { "Sources" }
        table class="rules" {
            thead { tr { th { "Log" } th { "Path" } th { "Records" } th { "Mediated" } th { "Observed" } th { "Reconstructed" } th { "Refused" } } }
            tbody {
                @for log in session["logs"].as_array().into_iter().flatten() {
                    tr {
                        td class="mono" {
                            a href=(format!("/app/record?session={}", encode_component(str_of(log.get("session_id"), "")))) {
                                (str_of(log.get("session_id"), ""))
                            }
                        }
                        td { (str_of(log.get("process_path"), "not recorded")) }
                        td { (u(log.get("records"))) }
                        td { (u(log.get("mediated"))) }
                        td { (u(log.get("observed"))) }
                        td { (u(log.get("reconstructed"))) }
                        td { (u(log.get("refused"))) }
                    }
                }
            }
        }
        p class="muted" { "Each log opens in Record, which shows every crossing." }

        h3 class="xh" { "Policy" }
        (policy_block(session))

        h3 class="xh" { "Context" }
        (context_block(&session["context"]))

        h3 class="xh" { "Reporting" }
        (reporting_block(session))

        h3 class="xh" { "Attention" }
        @if session["attention"].as_array().is_none_or(Vec::is_empty) {
            p class="muted" { "Nothing recorded needs attention." }
        } @else {
            table class="rules" {
                thead { tr { th { "Issue" } th { "Evidence" } th { "Next action" } th { "Record" } } }
                tbody {
                    @for item in session["attention"].as_array().into_iter().flatten() { (attention_row(item)) }
                }
            }
        }
    }
}

fn field(label: &str, value: &str) -> Markup {
    html! { div { dt { (label) } dd { (value) } } }
}

fn policy_block(session: &Value) -> Markup {
    let policy = &session["policy"];
    html! {
        dl class="cx-fields" {
            @for log in policy["comparison"]["logs"].as_array().into_iter().flatten() {
                div {
                    dt { "Loaded by " span class="mono" { (str_of(log.get("session_id"), "")) } }
                    dd {
                        @if let Some(identity) = log["identity"].as_str() { span class="mono" { (identity) } " — " }
                        span class=(if log["state"] == "superseded" { "stop" } else { "" }) {
                            (str_of(log.get("line"), ""))
                        }
                    }
                }
            }
            @for reason in policy["unavailable"].as_array().into_iter().flatten() {
                (field("Policy unavailable at a boundary", reason.as_str().unwrap_or("")))
            }
            @for sync in policy["syncs"].as_array().into_iter().flatten() {
                (field(
                    &format!("Policy refresh ({})", str_of(sync.get("trigger"), "trigger not recorded")),
                    &format!(
                        "{}; revision {} in force{}",
                        str_of(sync.get("outcome"), "outcome not recorded"),
                        match sync["applied"]["revision"].as_u64() {
                            Some(revision) => revision.to_string(),
                            None => "not recorded".to_owned(),
                        },
                        match sync["stale_since"].as_str() {
                            Some(since) => format!("; stale since {since}"),
                            None => String::new(),
                        },
                    ),
                ))
            }
            @if policy["syncs"].as_array().is_none_or(Vec::is_empty) {
                (field("Policy refresh", "none recorded; a local edge makes no management request"))
            }
        }
    }
}

/// The latest context snapshot: the API-reported counters apart from the
/// estimated inventory, and nothing summed across the two bases.
fn context_block(snapshot: &Value) -> Markup {
    if !snapshot.is_object() {
        return html! { p class="muted" { "No context snapshot recorded." } };
    }
    let inventory = &snapshot["inventory"];
    let names = |value: &Value| -> String {
        let listed: Vec<&str> = value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if listed.is_empty() {
            "none recorded".to_owned()
        } else {
            listed.join(", ")
        }
    };
    html! {
        dl class="cx-fields" {
            (field("Observed at", &when(snapshot.get("observed_at"))))
            @if let Some(model) = snapshot["model"].as_str() { (field("Model", model)) }
        }
        h4 { "API-reported counters" }
        dl class="cx-fields" {
            @for (key, label) in [("context_tokens", "Context tokens"), ("input_tokens", "Input tokens"),
                ("cache_read_input_tokens", "Cache read"), ("cache_creation_input_tokens", "Cache creation"),
                ("output_tokens", "Output tokens")] {
                @if let Some(count) = snapshot[key].as_u64() { (field(label, &count.to_string())) }
            }
            (field("Basis", str_of(snapshot.get("basis"), "not recorded")))
        }
        @if inventory.is_object() {
            h4 { "Estimated inventory (" (str_of(inventory.get("token_basis"), "basis not recorded")) ")" }
            dl class="cx-fields" {
                @for (category, figures) in inventory["categories"].as_object().into_iter().flatten() {
                    @if let Some(tokens) = figures["estimated_tokens"].as_u64() {
                        (field(category, &format!("{tokens} estimated tokens in {} records", figures["records"])))
                    }
                }
                @if let Some(compactions) = inventory["compactions"].as_u64() {
                    (field("Compactions", &compactions.to_string()))
                }
            }
            h4 { "Capabilities" }
            dl class="cx-fields" {
                @for (kind, listed) in inventory["available_on_demand"].as_object().into_iter().flatten() {
                    (field(&format!("Available on demand: {kind}"), &names(listed)))
                }
                (field("Definitions loaded", &names(&inventory["definitions_loaded"])))
                (field("Invoked", &{
                    let invoked: Vec<String> = inventory["invoked"]
                        .as_object()
                        .into_iter()
                        .flatten()
                        .filter(|(_, count)| count.as_u64().is_some_and(|count| count > 0))
                        .map(|(name, count)| format!("{name} ×{count}"))
                        .collect();
                    if invoked.is_empty() { "none recorded".to_owned() } else { invoked.join(", ") }
                }))
            }
        }
        h4 { "Unavailable" }
        dl class="cx-fields" {
            @for entry in snapshot["unavailable"].as_array().into_iter().flatten() {
                (field("Unavailable", entry.as_str().unwrap_or("")))
            }
        }
    }
}

fn reporting_block(session: &Value) -> Markup {
    let identity = &session["edge_identity"];
    html! {
        dl class="cx-fields" {
            @if identity.is_object() {
                (field("Hub", str_of(identity.get("hub"), "not recorded")))
                (field("Key", &format!(
                    "{} ({})",
                    str_of(identity.get("key_id"), "not recorded"),
                    str_of(identity.get("standing"), "standing not recorded"),
                )))
                @if let Some(until) = identity["listed_until"].as_str() { (field("Directory listing until", until)) }
                @if let Some(reason) = identity["unlisted"].as_str() { (field("Directory listing", &format!("unlisted: {reason}"))) }
            } @else {
                (field("Edge identity", "none recorded; the edge was not enrolled when the session started"))
            }
            @if u(session.get("evidence_gaps")) > 0 {
                (field("Evidence gaps", &u(session.get("evidence_gaps")).to_string()))
            }
            @if u(session.get("unreadable")) > 0 {
                (field("Unreadable lines", &u(session.get("unreadable")).to_string()))
            }
        }
        p class="muted" { "Delivery to a receiver is reported for the edge on " a href="/app" { "Overview" } "." }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(answer: Option<bool>) -> LivenessProbe {
        Arc::new(move |_, _| answer)
    }

    fn session(worked: bool, ended: bool, process: bool) -> Value {
        json!({
            "id": "s-1",
            "host": "codex",
            "worked": worked,
            "first": "2026-09-14T02:00:00Z",
            "ended": if ended { json!({"timestamp": "2026-09-14T03:00:00Z", "reason": "clear"}) } else { Value::Null },
            "host_process": if process {
                json!({"pid": 4242, "started_at": "2026-09-14T01:59:58Z", "command": "codex"})
            } else { Value::Null },
            "join": {"basis": "single log, join unavailable", "detail": "no host_process record"},
            "policy": {"syncs": [], "divergent_members": []},
            "logs": [],
        })
    }

    /// Establishes the state rules over the fake probe: present with work is
    /// seen working, present without is running, absent with no end record
    /// is gone without a session end, and neither a missing probe nor a
    /// missing record is ever reported as running.
    #[test]
    fn the_probe_and_the_end_record_decide_the_state_and_nothing_else_does() {
        let state = |session: &Value, probe: Option<&LivenessProbe>| {
            liveness_of(session, probe)["state"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let present = probe(Some(true));
        let absent = probe(Some(false));
        let unreadable = probe(None);
        assert_eq!(
            state(&session(true, false, true), Some(&present)),
            "seen_working"
        );
        assert_eq!(
            state(&session(false, false, true), Some(&present)),
            "running"
        );
        assert_eq!(
            state(&session(true, false, true), Some(&absent)),
            "gone_without_session_end"
        );
        assert_eq!(
            state(&session(true, false, true), Some(&unreadable)),
            "unavailable"
        );
        assert_eq!(state(&session(true, false, true), None), "unavailable");
        // No host_process record: the probe is never asked.
        assert_eq!(
            state(&session(true, false, false), Some(&present)),
            "unavailable"
        );
        // The host reported the end; a process still present is another session.
        assert_eq!(state(&session(true, true, true), Some(&present)), "ended");
    }

    /// Establishes the idle item's three conditions: no work, the probe says
    /// present, and the first record is more than an hour old.
    #[test]
    fn a_running_process_that_never_worked_is_listed_after_an_hour() {
        let now = DateTime::parse_from_rfc3339("2026-09-17T02:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let listed = |worked: bool, answer: Option<bool>, now: DateTime<Utc>| {
            let probe = probe(answer);
            let mut session = session(worked, false, true);
            session["liveness"] = liveness_of(&session, Some(&probe));
            session_attention(&session, now)
                .iter()
                .any(|item| item["kind"] == "idle_integration")
        };
        assert!(listed(false, Some(true), now));
        assert!(
            !listed(true, Some(true), now),
            "a session that worked is not idle"
        );
        assert!(
            !listed(false, Some(false), now),
            "a process that is gone is not running"
        );
        assert!(!listed(false, None, now), "unknown liveness is not running");
        let early = DateTime::parse_from_rfc3339("2026-09-14T02:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(!listed(false, Some(true), early), "under an hour old");
    }

    #[test]
    fn a_host_outside_the_path_table_is_said_to_be_outside_it() {
        let sessions = json!([{
            "host": "some-new-host",
            "logs": [{"session_id": "local-1-2", "host": "some-new-host", "events": {"crossing_mediated": 1}}],
        }]);
        let coverage = coverage(&sessions);
        assert_eq!(coverage[0]["in_path_table"], false);
        assert_eq!(coverage[0]["paths"][0]["shown"], true);
        assert!(coverage[0]["paths"][1]["no_path"].is_null());
    }
}
