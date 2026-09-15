//! The operator console: a maud-rendered app shell.
//!
//! Five screens behind a persistent sidebar — Overview, Record, Policy,
//! Sources and Compare — each a bookmarkable route. Every screen carries a
//! level-one heading, a line naming the record it was read from and when, its
//! values with labels, and an explicit state where there is nothing to show;
//! no screen explains itself in paragraphs.
//!
//! Rendering is [`maud`]: structured markup checked for tag nesting at compile
//! time, rendered server-side into the one binary with no build step.
//!
//! Every value comes from the same evidence store the JSON API serves, so this
//! module is presentation only: it takes the store's JSON projections and
//! lays them out. Unknown stays the word "unknown", never zero; a field the
//! record does not carry is absent from the screen, never invented
//! (`super::html` holds that discipline for the shared formatters).

use maud::{DOCTYPE, Markup, html};
use serde_json::Value;

/// The sidebar sections, in order. Each is a route (`/app`, `/app/record`, …)
/// so the view is bookmarkable and the server renders the active one.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Overview,
    Record,
    Policy,
    Sources,
    Compare,
    Budget,
}

impl Section {
    pub fn slug(self) -> &'static str {
        match self {
            Section::Overview => "",
            Section::Record => "record",
            Section::Policy => "policy",
            Section::Sources => "sources",
            Section::Compare => "compare",
            Section::Budget => "budget",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Section::Overview => "Overview",
            Section::Record => "Record",
            Section::Policy => "Policy",
            Section::Sources => "Sources",
            Section::Compare => "Compare",
            Section::Budget => "Budget",
        }
    }

    fn href(self) -> String {
        match self {
            Section::Overview => "/app".to_owned(),
            other => format!("/app/{}", other.slug()),
        }
    }

    const ALL: [Section; 6] = [
        Section::Overview,
        Section::Record,
        Section::Policy,
        Section::Sources,
        Section::Compare,
        Section::Budget,
    ];
}

/// Where a screen's values were read from and when, stated on the screen so
/// it stands alone: the sessions directory the index was derived from, and
/// the time of the read.
#[derive(Clone, Copy)]
pub struct ReadFrom<'a> {
    pub sessions: &'a str,
    pub at: &'a str,
}

// One entry point per section: each gathers only its own data in the caller
// and renders the shell around its screen. Keeping them separate is what lets
// `serve` fetch just what a section needs rather than everything for every view.

pub fn overview_page(status: &Value, content: &Value, read: ReadFrom<'_>) -> String {
    shell(Some(Section::Overview), overview(status, content, read)).into_string()
}

/// The Record screen. `selected` is one session rendered into the pane by the
/// server, for a `?session=` link followed with or without scripts.
pub fn record_page(
    sessions: &Value,
    selected: Option<(&str, &Value)>,
    read: ReadFrom<'_>,
) -> String {
    shell(Some(Section::Record), record(sessions, selected, read)).into_string()
}

/// The Policy screen as a whole page, with an outcome notice when a write
/// just happened (`kind`, `text`).
pub fn policy_page(
    policy: &Value,
    rules: &Value,
    notice: Option<(&str, &str)>,
    at: &str,
) -> String {
    shell(
        Some(Section::Policy),
        policy_screen(policy, rules, notice, at),
    )
    .into_string()
}

/// The Policy screen below its top bar, for the htmx swap after a write.
pub fn policy_fragment(policy: &Value, rules: &Value, notice: Option<(&str, &str)>) -> String {
    policy_body(policy, rules, notice).into_string()
}

pub fn sources_page(providers: &[Value]) -> String {
    shell(Some(Section::Sources), sources(providers)).into_string()
}

pub fn compare_page(query: Option<&str>, results: &[Value], providers: &[Value]) -> String {
    shell(Some(Section::Compare), compare(query, results, providers)).into_string()
}

/// The detail pane for one session, loaded by htmx into the Record screen's
/// pane when a session in the rail is chosen. Returned as a fragment, not a
/// whole page.
pub fn session_detail(id: &str, records: &Value) -> String {
    detail_body(id, records).into_string()
}

/// An error answered as a page: the shell with no section active, the status
/// as the heading and the reason beneath it, so a person who followed a bad
/// link reads a page in the console's own language rather than a JSON body.
pub fn error_page(status: u16, detail: &str) -> String {
    let title = match status {
        400 => "Bad request",
        404 => "Not found",
        _ => "Error",
    };
    shell(
        None,
        html! {
            header class="topbar" { h1 { (status) " " (title) } }
            section class="screen" {
                p class="muted" { (detail) }
                p class="muted" { a href="/app" { "Overview" } }
            }
        },
    )
    .into_string()
}

/// The document shell: sidebar plus the active section's content.
fn shell(active: Option<Section>, main: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en-GB" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Common Measure Console" }
                link rel="stylesheet" href="/styles.css";
                // A write answers with its outcome's own status, and the
                // answer is the screen to read whatever the status was.
                meta name="htmx-config" content=r#"{"responseHandling":[{"code":"204","swap":false},{"code":"[2345]..","swap":true},{"code":"...","swap":true}]}"#;
                script src="/htmx.min.js" defer {}
            }
            body {
                a class="skip" href="#main" { "Skip to content" }
                div class="app" {
                    (sidebar(active))
                    main class="work" id="main" { (main) }
                }
            }
        }
    }
}

fn sidebar(active: Option<Section>) -> Markup {
    html! {
        aside class="side" aria-label="Console" {
            div class="brand" {
                div class="wordmark" { "Common Measure" }
                div class="brand-sub" { "Console" }
            }
            nav class="nav" aria-label="Sections" {
                @for section in Section::ALL {
                    a class=(nav_class(Some(section) == active)) href=(section.href())
                        aria-current=[(Some(section) == active).then_some("page")] {
                        (section.label())
                    }
                }
            }
        }
    }
}

fn nav_class(active: bool) -> &'static str {
    if active { "nav-item on" } else { "nav-item" }
}

/// A screen's top bar: its title and the line saying what it was read from.
fn topbar(title: &str, read: Markup) -> Markup {
    html! {
        header class="topbar" {
            h1 { (title) }
            p class="lede" { (read) }
        }
    }
}

/// The Overview screen: the counts, the most recent crossings, the crossings
/// by engagement, and the hub.
fn overview(status: &Value, content: &Value, read: ReadFrom<'_>) -> Markup {
    let engagements = status
        .get("engagements")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // Totals across engagements. A count that is not a number stays out of the
    // sum rather than being read as zero.
    let sum = |key: &str| -> u64 {
        engagements
            .iter()
            .filter_map(|e| e.get(key).and_then(Value::as_u64))
            .sum()
    };
    let witnessed = sum("witnessed");
    let reconstructed = sum("reconstructed");
    let refused = sum("refused");
    let egress = status.get("egress").cloned().unwrap_or_default();
    let cleared = egress.get("delivered").and_then(Value::as_u64).unwrap_or(0);
    let receiver = egress.get("receiver").and_then(Value::as_str);
    let key_id = egress.get("key_id").and_then(Value::as_str);
    let key_standing = egress.get("key_standing").and_then(Value::as_str);

    let recent: Vec<&Value> = content
        .as_array()
        .map(|list| list.iter().take(5).collect())
        .unwrap_or_default();

    html! {
        (topbar("Overview", html! {
            "Read from " span class="mono" { (read.sessions) } " at " (read.at) "."
        }))
        section class="screen" {
            div class="metrics" {
                (metric(&(witnessed + reconstructed).to_string(), "crossings recorded", ""))
                (metric(&witnessed.to_string(), "witnessed by Common Measure", "act"))
                (metric(&refused.to_string(), "refused by your policy · all recorded history", "stop"))
                (metric(&cleared.to_string(), "cleared to the hub", ""))
            }
            div class="cards grid-2" {
                div class="card" {
                    h2 { "Recent crossings" a href="/app/record" { "Record" } }
                    @if recent.is_empty() {
                        p class="muted" { "No crossings recorded." }
                    } @else {
                        div class="rows" {
                            @for row in &recent {
                                div class="row" {
                                    (host_cell(row))
                                    span class="eng" { (engagements_of(row)) }
                                }
                            }
                        }
                    }
                }
                div class="card" {
                    h2 { "Crossings by engagement" }
                    @if engagements.is_empty() {
                        p class="muted" { "No crossings recorded." }
                    } @else {
                        div class="rows" {
                            @for e in &engagements {
                                div class="row" {
                                    span class="host" { (str_of(e.get("engagement"), "unattributed")) }
                                    span class="eng" {
                                        span class="mono" {
                                            (e.get("witnessed").and_then(Value::as_u64).unwrap_or(0)
                                                + e.get("reconstructed").and_then(Value::as_u64).unwrap_or(0))
                                        }
                                        " crossings"
                                    }
                                }
                            }
                        }
                        p class="callout" { "Attributed by the working directory each crossing ran in." }
                    }
                }
                div class="card" {
                    h2 { "Hub" }
                    dl class="kv" {
                        div {
                            dt { "Receiver" }
                            dd {
                                @match receiver {
                                    Some(receiver) => { span class="mono" { (receiver) } }
                                    None => { "none configured; nothing leaves this machine" }
                                }
                            }
                        }
                        div {
                            dt { "Edge key" }
                            dd {
                                @match (key_id, key_standing) {
                                    (Some(key_id), Some(standing)) => {
                                        span class="mono" { (key_id) } ", " (standing) "."
                                    }
                                    _ => { "Not enrolled with a hub: no edge key." }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The Record screen: a session rail on the left, and a pane on the right that
/// holds the chosen session's crossings. The rail is for finding a session;
/// the pane is for reading one, so the grade breakdown lives in the pane.
fn record(sessions: &Value, selected: Option<(&str, &Value)>, read: ReadFrom<'_>) -> Markup {
    let list = sessions.as_array().cloned().unwrap_or_default();
    // Group by engagement, preserving first-seen order, so the rail reads as
    // the operator's work by where it ran rather than one undifferentiated list.
    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, Vec<Value>> =
        std::collections::HashMap::new();
    for session in &list {
        let engagement = str_of(session.get("engagement"), "unattributed").to_owned();
        if !groups.contains_key(&engagement) {
            order.push(engagement.clone());
        }
        groups.entry(engagement).or_default().push(session.clone());
    }
    let selected_id = selected.map(|(id, _)| id);
    html! {
        (topbar("Record", html! {
            "Read from " span class="mono" { (read.sessions) } " at " (read.at) "."
        }))
        section class="screen record" {
            div class="md" {
                aside class="md-rail" aria-label="Sessions" {
                    @if list.is_empty() {
                        p class="muted" { "No sessions recorded." }
                    } @else {
                        @for engagement in &order {
                            h2 class="md-group" { (engagement) }
                            @for session in &groups[engagement] {
                                (session_row(session, selected_id))
                            }
                        }
                    }
                    p class="note" {
                        "A Claude Code session's mediated crossings are recorded under a separate "
                        span class="mono" { "local-…" }
                        " session of the MCP server's own, beside the host session's observed record."
                    }
                }
                section class="md-detail" id="detail" aria-label="Session" {
                    @match selected {
                        Some((id, records)) => (detail_body(id, records)),
                        None => p class="muted pad" { "No session selected." },
                    }
                }
            }
        }
    }
}

/// One rail row: a link to the session, which the server renders into the
/// pane and which htmx swaps in without a page load. Percent-encoded link,
/// not HTML-escaped — a session id is a transcript file stem, so the encoder
/// emits only unreserved bytes.
fn session_row(session: &Value, selected: Option<&str>) -> Markup {
    let id = str_of(session.get("session_id"), "unknown");
    let encoded = super::form::encode_component(id);
    let page = format!("/app/record?session={encoded}");
    // "Crossed" is grade-neutral: observed and mediated are both crossings
    // that happened, which is what the store's `observed` and `mediated`
    // counts are (`store.rs`, `into_json`). A fully mediated session is not
    // "0 crossed".
    let witnessed = u(session.get("observed")) + u(session.get("mediated"));
    let refused = u(session.get("refused"));
    // A breach is a crossing a policy rule failed but the mode let through —
    // the store counts a record whose `payload.breach` names the rule.
    let breached = u(session.get("breached"));
    html! {
        a class="srow" href=(page)
            hx-get=(format!("/app/fragments/session/{encoded}"))
            hx-target="#detail" hx-swap="innerHTML" hx-push-url=(page)
            aria-current=[(selected == Some(id)).then_some("true")]
        {
            span class="srow-top" {
                span class="mono" { (id) }
                small { (str_of(session.get("host"), "")) }
            }
            span class="srow-meta" {
                (witnessed) " crossed"
                @if refused > 0 { " · " (refused) " refused" }
                @if breached > 0 { " · " (breached) " breached" }
                " · " (when(session.get("last")))
            }
        }
    }
}

/// The detail pane body: the session's facts, then its crossings each shown as
/// itself — source, grade, licence, grounded, the hashes and status the record
/// carries — with a refused one marked. It opens with a heading that takes
/// focus when htmx swaps it in, so a keyboard or screen-reader user lands on
/// the session they chose.
fn detail_body(id: &str, records: &Value) -> Markup {
    let empty = Vec::new();
    let recs = records.as_array().unwrap_or(&empty);
    if let Some(error) = records.get("error").and_then(Value::as_str) {
        return html! {
            h2 class="detail-head" tabindex="-1" autofocus { "Session " span class="mono id" { (id) } }
            p class="muted" { (error) }
        };
    }
    let crossings: Vec<&Value> = recs
        .iter()
        .filter(|r| {
            r.get("event")
                .and_then(Value::as_str)
                .is_some_and(|e| e.starts_with("crossing_"))
        })
        .collect();
    let count_where = |pred: &dyn Fn(&Value) -> bool| crossings.iter().filter(|r| pred(r)).count();
    let witnessed = count_where(&|r| grade_of(r) != "reconstructed");
    let reconstructed = count_where(&|r| grade_of(r) == "reconstructed");
    let grounded = count_where(&|r| {
        r.get("payload")
            .and_then(|p| p.get("grounded"))
            .and_then(Value::as_bool)
            == Some(true)
    });
    let refused = count_where(&|r| refusal_of(r).is_some());
    // The store's rule, mirrored: any record whose `payload.breach` is a
    // string carried a breach (`store.rs`, `SessionSummary::absorb`). In
    // `prefer` mode a denied host crosses anyway and is recorded this way, so
    // the count is a stop-fact like refused, not a neutral one.
    let breached = recs
        .iter()
        .filter(|r| {
            r.get("payload")
                .and_then(|p| p.get("breach"))
                .is_some_and(Value::is_string)
        })
        .count();

    html! {
        h2 class="detail-head" tabindex="-1" autofocus { "Session " span class="mono id" { (id) } }
        div class="facts" {
            (fact("witnessed", &witnessed.to_string(), false))
            (fact("reconstructed", &reconstructed.to_string(), false))
            (fact("grounded", &grounded.to_string(), false))
            (fact("refused", &refused.to_string(), refused > 0))
            (fact("breached", &breached.to_string(), breached > 0))
        }
        h3 class="xh" { "Crossings" }
        @if crossings.is_empty() {
            p class="muted" { "This session recorded no crossings." }
        } @else {
            @for crossing in &crossings { (crossing_card(crossing, recs)) }
        }
        @if let Some(line) = other_records(&recs.iter().collect::<Vec<_>>()) {
            p class="muted" { (line) }
        }
    }
}

/// What the session recorded that is not a crossing, in a line, or `None`
/// when there is nothing.
///
/// A session with no crossing is not an empty session: a turn boundary and an
/// unreadable line are records, and the store counts both. Saying so here
/// stops "no crossings" being read as "nothing happened", and stops a session
/// whose log this console could not parse looking like a quiet one.
fn other_records(records: &[&Value]) -> Option<String> {
    let count = |wanted: &[&str]| {
        records
            .iter()
            .filter(|record| {
                record
                    .get("event")
                    .and_then(Value::as_str)
                    .is_some_and(|event| wanted.contains(&event))
            })
            .count()
    };
    let turns = count(&["turn_started", "turn_completed"]);
    let unreadable = count(&["unreadable"]);
    let mut parts = Vec::new();
    if turns > 0 {
        parts.push(match turns {
            1 => "1 turn boundary".to_owned(),
            many => format!("{many} turn boundaries"),
        });
    }
    if unreadable > 0 {
        parts.push(match unreadable {
            1 => "1 line this console could not read".to_owned(),
            many => format!("{many} lines this console could not read"),
        });
    }
    (!parts.is_empty()).then(|| format!("Also recorded: {}.", parts.join(", ")))
}

/// One crossing, shown as itself. A refused crossing is marked and gives its
/// reason; every other shows its grade, whether it grounded, and the fields
/// the record carries: the hash of the bytes the origin served, the hash of
/// the text delivered, the HTTP status and the identity the request presented.
/// A field the record does not carry is absent, never "unknown" or zero: an
/// observed crossing carries a content hash and nothing else of these.
fn crossing_card(record: &Value, records: &[Value]) -> Markup {
    let payload = record.get("payload").unwrap_or(&Value::Null);
    let url = str_of(payload.get("url"), "");
    let host = payload
        .get("host_name")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| host_of(url));
    let grounded = payload.get("grounded").and_then(Value::as_bool) == Some(true);
    let licence = payload
        .get("licence")
        .and_then(|l| l.get("state"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    match refusal_of(record) {
        Some(reason) => html! {
            div class="crossing refused" {
                div class="cx-line1" {
                    span class="url mono" { (url) }
                    span class="cx-badges" { span class="badge b-refused" { "refused" } }
                }
                div class="cx-line2" { span class="stop" { (reason) } }
                (record_fields(payload))
                (crossing_evidence(record, records))
            }
        },
        None => {
            let grade = grade_of(record);
            let challenge = challenge_of(record);
            html! {
                div class="crossing" {
                    div class="cx-line1" {
                        span class="url mono" { (url) }
                        span class="cx-badges" {
                            span class=(grade_badge_class(grade)) { (grade) }
                            @if grounded { span class="badge b-grounded" { "grounded" } }
                            @if challenge.is_some() { span class="badge b-refused" { "challenged" } }
                        }
                    }
                    div class="cx-line2" {
                        span { (host) }
                        span class="mono" { "licence: " (licence) }
                    }
                    (record_fields(payload))
                    (crossing_evidence(record, records))
                    // The origin's own refusal, kept apart from the operator's
                    // policy refusal above and from a transport failure: all
                    // three leave a crossing with no content, and a reader has
                    // to be able to tell whose decision it was
                    // (`docs/contracts/session-evidence.md`).
                    @if let Some(challenge) = &challenge {
                        div class="cx-line2" { span class="stop" { (challenge) } }
                    }
                }
            }
        }
    }
}

/// The record's own fields on a crossing, as labelled rows, each present only
/// when the record carries it (`docs/contracts/session-evidence.md` §Crossing).
/// The labels are the words the product site uses for the same two hashes.
fn record_fields(payload: &Value) -> Markup {
    let retrieved = payload.get("retrieved_hash").and_then(Value::as_str);
    let content = payload.get("content_hash").and_then(Value::as_str);
    let status = payload.get("http_status").and_then(Value::as_u64);
    let identity = identity_of(payload);
    html! {
        @if retrieved.is_some() || content.is_some() || status.is_some() || identity.is_some() {
            dl class="cx-fields" {
                @if let Some(hash) = retrieved {
                    div { dt { "bytes received" } dd class="mono" { (hash) } }
                }
                @if let Some(hash) = content {
                    div { dt { "text read" } dd class="mono" { (hash) } }
                }
                @if let Some(status) = status {
                    div { dt { "status" } dd class="mono" { (status) } }
                }
                @if let Some(identity) = &identity {
                    div { dt { "identity" } dd { (identity) } }
                }
            }
        }
    }
}

/// Structured evidence stays in its recorded shape. Missing fields produce no
/// row; explicit nulls state absence and never become measurements.
fn evidence_value(value: &Value) -> Markup {
    match value {
        Value::Object(fields) => html! { dl {
            @for (key, value) in fields { dt { (key.replace('_', " ")) } dd { (evidence_value(value)) } }
        } },
        Value::Array(values) => {
            html! { ul { @for value in values { li { (evidence_value(value)) } } } }
        }
        Value::String(value) => html! { (value) },
        Value::Null => html! { "absent" },
        other => html! { (other) },
    }
}

fn crossing_evidence(crossing: &Value, records: &[Value]) -> Markup {
    let payload = &crossing["payload"];
    // Concurrent writers may reuse sequence numbers. Only a unique preceding
    // reference to this host can establish which manifest the crossing used.
    let manifest = payload.get("manifest_record").and_then(|reference| {
        let mut candidates = records
            .iter()
            .take_while(|r| !std::ptr::eq(*r, crossing))
            .filter(|r| r["event"] == "manifest_resolved" && r.get("seq") == Some(reference));
        let record = candidates.next()?;
        (candidates.next().is_none()
            && record["payload"]["host"].as_str()
                == payload["url"]
                    .as_str()
                    .map(commonmeasure_harness::grounding::host_of)
                    .as_deref())
        .then_some(record)
    });
    html! {
        dl class="cx-fields" {
            @for (key, label) in [("declarations", "Source declarations"), ("named_by", "Named by"),
                ("content_telemetry_id", "Content-Telemetry-ID"), ("allowance", "Allowance"),
                ("breach", "Policy breach"), ("failure", "Transport failure")] {
                @if let Some(value) = payload.get(key).filter(|v| !v.is_null()) {
                    div { dt { (label) } dd { (evidence_value(value)) } }
                }
            }
            @if payload["declarations"].get("reporting").is_some_and(|r| r.is_object() && r.get("receiver").is_none_or(Value::is_null)) {
                div { dt { "Reporting ruling" } dd { "Reporting receiver: absent from this ruling." } }
            }
            @if let Some(reference) = payload.get("manifest_record").filter(|v| !v.is_null()) {
                div { dt { "Manifest record" } dd {
                    (reference)
                    @if let Some(record) = manifest {
                        (evidence_value(&record["payload"]))
                    } @else {
                        p class="muted" { "The referenced manifest record is unavailable or ambiguous in this session." }
                    }
                } }
            }
        }
    }
}

/// Render the budget API projection without recomputing spend or policy caps.
pub fn budget_page(budget: &Value, read: ReadFrom<'_>) -> String {
    shell(Some(Section::Budget), html! {
        (topbar("Budget", html! { "Read from " (read.sessions) " at " (read.at) "." }))
        section class="screen" {
            h2 { "Recorded footprint by engagement" }
            p class="callout" { (str_of(budget["acquisition_charge"].get("reason"), "")) }
            @for row in budget["engagements"].as_array().into_iter().flatten() {
                div class="card" {
                    h3 { (str_of(row.get("engagement"), "")) }
                    dl class="cx-fields" {
                        @for (key, label) in [("witnessed", "Witnessed crossings"), ("reconstructed", "Reconstructed crossings"),
                            ("refused", "Refused crossings"), ("estimated_tokens", "Estimated tokens"),
                            ("token_basis", "Token basis"), ("estimated_tokens_by_basis", "Estimates by basis"),
                            ("total_withheld", "Why the total is withheld"), ("crossings_without_estimate", "Crossings without an estimate"),
                            ("declared_cap", "Declared acquisition cap (not a periodic allowance)")] {
                            @if let Some(value) = row.get(key) {
                                div { dt { (label) } dd { (evidence_value(value)) } }
                            }
                        }
                        div { dt { "Acquisition charge" } dd { "Not recorded per engagement." } }
                    }
                }
            }
            h2 { "Principal allowances" }
            p class="callout" { "Declarations from policy.json; standing from allowance/ledger.ndjson. Principal allowances are independent of the engagement filter." }
            (evidence_value(&budget["allowances"]))
        }
    }).into_string()
}

/// Render the same comparison outcome returned to a JSON client.
pub fn compare_answer_page(result: &Value, providers: &[Value]) -> String {
    shell(Some(Section::Compare), html! {
        p class="callout" role="status" data-outcome=(str_of(result.get("kind"), "")) {
            (str_of(result.get("notice"), ""))
        }
        (compare(result["query"].as_str(), result["results"].as_array().map(Vec::as_slice).unwrap_or(&[]), providers))
    }).into_string()
}

/// The Policy screen: what governs the next crossing, and the three edits the
/// console makes to it. Every form states the revision it was rendered from,
/// so a save against a file that changed underneath is refused with both
/// versions stated (`super::edit`). The forms post through htmx and swap the
/// whole screen with one rendered from a fresh read, and they carry a plain
/// `action` as well so they work with no script at all.
fn policy_screen(policy: &Value, rules: &Value, notice: Option<(&str, &str)>, at: &str) -> Markup {
    let source = str_of(policy.get("source"), "policy.json");
    let revision = policy.get("revision").and_then(Value::as_str);
    html! {
        (topbar("Policy", html! {
            "Read from " span class="mono" { (source) }
            @if let Some(revision) = revision { ", revision " span class="mono" { (short(revision)) } }
            " at " (at) "."
        }))
        (policy_body(policy, rules, notice))
    }
}

/// The screen below the top bar: what an htmx write swaps in place.
fn policy_body(policy: &Value, rules: &Value, notice: Option<(&str, &str)>) -> Markup {
    let declared = policy
        .get("declared")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let source = str_of(policy.get("source"), "your policy file");
    let revision = str_of(policy.get("revision"), "");
    let modes = policy
        .get("modes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // The top-level mode governs any directory no scope matches: the scope-null
    // entry in the modes list.
    let top_mode = modes
        .iter()
        .find(|m| m.get("scope").map(Value::is_null).unwrap_or(false))
        .and_then(|m| m.get("mode"))
        .and_then(Value::as_str)
        .unwrap_or("observe");
    // How many engagements clear any telemetry to the hub.
    let cleared = policy
        .get("engagements")
        .and_then(Value::as_array)
        .map(|engs| {
            engs.iter()
                .filter(|e| {
                    e.get("stances")
                        .and_then(Value::as_array)
                        .map(|st| {
                            st.iter().any(|s| {
                                s.get("allow_telemetry_egress") == Some(&Value::Bool(true))
                            })
                        })
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0);
    let divergences = policy
        .get("engagement_divergences")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let error = policy.get("error").and_then(Value::as_str);

    html! {
        section class="screen" id="policy-screen" {
            @if let Some((kind, text)) = notice {
                div class=(format!("notice {kind}")) role="status" data-outcome=(kind) { (text) }
            }
            div class="cards stack" {
                @if let Some(error) = error {
                    div class="card" {
                        h2 { "Policy" }
                        p class="muted" { "The policy file did not load, so nothing here can be edited until it is repaired by hand: " (error) }
                    }
                } @else if !declared {
                    div class="card" {
                        h2 { "Policy" }
                        p class="muted" {
                            "No policy is declared, so Common Measure records everything and refuses nothing. Declare one in "
                            span class="mono" { (source) }
                            " to start refusing crossings before they reach your agent. The console never creates a policy file."
                        }
                    }
                } @else {
                    div class="card" {
                        h2 { "Mode" span class="muted-inline" { "for every directory no scope matches" } }
                        form class="dial-form" method="post" action="/app/policy/mode"
                             hx-post="/app/policy/mode" hx-target="#policy-screen" hx-swap="outerHTML" {
                            input type="hidden" name="scope" value="";
                            input type="hidden" name="revision" value=(revision);
                            fieldset class="dial-options" {
                                legend class="sr-only" { "Mode for every directory no scope matches" }
                                (dial_option("observe", top_mode, "Everything is recorded; nothing is blocked."))
                                (dial_option("prefer", top_mode, "Rules steer selection, but a failing crossing is admitted and flagged."))
                                (dial_option("strict", top_mode, "A crossing that fails a rule is refused before it reaches your agent."))
                            }
                            div class="rule-actions" {
                                button type="button" class="btn quiet"
                                       hx-get="/app/policy/forecast" hx-include="closest form" hx-vals=r#"{"action":"mode"}"#
                                       hx-target="#forecast-mode" hx-swap="innerHTML" { "Forecast over the record" }
                                button type="submit" class="btn" { "Save mode" }
                            }
                        }
                        div class="forecast" id="forecast-mode" role="status" {}
                        p class="callout" { (super::edit::REACH) }
                    }
                    div class="card" {
                        h2 {
                            "Scopes"
                            span class="muted-inline" { (cleared) " clearing to the hub" }
                        }
                        div class="table-wrap" {
                            table class="rules" {
                                thead { tr {
                                    th { "Directory" } th { "Mode" } th { "Governs" } th { "Directories" } th { "Denied hosts" } th { "Access rules" }
                                    th { "Policy identity" }
                                } }
                                tbody { @for m in &modes { (scope_row(m, revision)) } }
                            }
                        }
                        p class="muted" {
                            "Policy identity is resolved for the principal this console runs as ("
                            (str_of(policy.get("identity_principal").and_then(|p| p.get("name")), "unknown"))
                            ", basis "
                            (str_of(policy.get("identity_principal").and_then(|p| p.get("basis")), "unknown"))
                            "). A session under another principal resolves its own identity, which its mediated crossings record."
                        }
                        p class="callout" {
                            "Scopes resolve by the working directory a crossing ran in, first match wins; the mode decides whether a failing crossing is refused. A scope marked inherited follows the top-level value until it states its own."
                        }
                    }
                    div class="card" {
                        h2 { "Where the two engagement names differ" }
                        @if divergences.is_empty() {
                            p class="muted" { "Every scope's governing engagement, declared in the policy, matches the reported engagement the attribution rules resolve for its directories." }
                        } @else {
                            div class="table-wrap" {
                                table class="rules" id="divergences" {
                                    thead { tr {
                                        th { "Scope" } th { "Governing (policy)" } th { "Reported (attribution)" } th { "Telemetry egress" }
                                    } }
                                    tbody { @for d in &divergences { (divergence_row(d)) } }
                                }
                            }
                            p class="callout" {
                                "The governing engagement is what the runtime enforces and the relay reports under; the reported engagement is what this console counts work under. Neither overrides the other. Edit the scope in the policy file or the attribution rule below until they agree."
                            }
                        }
                    }
                    div class="card" {
                        h2 { "Block a host" span class="muted-inline" { "adds to a scope's denied-host set" } }
                        form class="draft" method="post" action="/app/policy/deny"
                             hx-post="/app/policy/deny" hx-target="#policy-screen" hx-swap="outerHTML" {
                            input type="hidden" name="revision" value=(revision);
                            input type="hidden" name="action" value="deny";
                            div class="fields" {
                                label class="field" { "Scope" (scope_select(&modes)) }
                                label class="field" { "Host" input type="text" name="host" class="mono" placeholder="tracker.example" required; }
                            }
                            div class="rule-actions" {
                                button type="button" class="btn quiet"
                                       hx-get="/app/policy/forecast" hx-include="closest form"
                                       hx-target="#forecast-deny" hx-swap="innerHTML" { "Forecast over the record" }
                                button type="submit" class="btn" { "Block" }
                            }
                        }
                        div class="forecast" id="forecast-deny" role="status" {}
                        p class="callout" { "A denied host is refused in strict mode and carried with the breach recorded in observe and prefer. A scope that inherits the top-level constraints keeps them on its first write and stops inheriting; the save says so." }
                    }
                    div class="card" {
                        h2 { "Draft an access rule" span class="muted-inline" { "forecast only; the rule is added to the policy file by hand" } }
                        form class="draft" hx-get="/app/policy/forecast" hx-target="#forecast-rule" hx-swap="innerHTML"
                             method="get" action="/app/policy/forecast" {
                            div class="fields" {
                                label class="field" { "Scope" (scope_select(&modes)) }
                                label class="field" { "Host pattern" input type="text" name="host" class="mono" placeholder="*.example.com" required; }
                                label class="field" { "Action"
                                    select name="action" {
                                        option value="refuse" { "refuse" }
                                        option value="allow" { "allow" }
                                        option value="require_licence" { "require_licence" }
                                        option value="require_mediation" { "require_mediation" }
                                    }
                                }
                                label class="field" { "Licence (require_licence)" input type="text" name="licence" class="mono" placeholder="rsl:publisher/2026"; }
                            }
                            div class="rule-actions" {
                                button type="submit" class="btn quiet" { "Forecast over the record" }
                            }
                        }
                        div class="forecast" id="forecast-rule" role="status" {}
                        p class="callout" { "Access rules are read in order and the first match decides. The forecast appends the draft after the scope's existing rules; a rule that must come first is an edit to the file." }
                    }
                }
                (attribution_card(rules))
            }
        }
    }
}

/// One radio in the mode dial, with the consequence of choosing it beside it.
fn dial_option(mode: &str, active: &str, consequence: &str) -> Markup {
    html! {
        label class="dial-option" {
            input type="radio" name="mode" value=(mode) checked[mode == active];
            span { (mode) small { " — " (consequence) } }
        }
    }
}

/// The scope chooser a draft form carries: every declared scope in its
/// declared order, then the top-level policy, which the form posts as an
/// empty scope.
fn scope_select(modes: &[Value]) -> Markup {
    html! {
        select name="scope" {
            @for m in modes {
                @match m.get("scope").and_then(Value::as_str) {
                    Some(scope) => option value=(scope) { (scope) },
                    None => option value="" { "everything else (top level)" },
                }
            }
        }
    }
}

fn scope_row(m: &Value, revision: &str) -> Markup {
    let mode = str_of(m.get("mode"), "observe");
    let inherited = m.get("declares_mode") == Some(&Value::Bool(false));
    let inherits_constraints = m.get("declares_constraints") == Some(&Value::Bool(false));
    let scope = m.get("scope").and_then(Value::as_str);
    let named = scope.unwrap_or("everything else");
    let denied: Vec<&str> = m
        .get("denied_hosts")
        .and_then(Value::as_array)
        .map(|hosts| hosts.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let access: Vec<&Value> = m
        .get("access_rules")
        .and_then(Value::as_array)
        .map(|rules| rules.iter().collect())
        .unwrap_or_default();
    html! {
        tr {
            td class="mono" { (named) }
            td {
                form class="inline" method="post" action="/app/policy/mode"
                     hx-post="/app/policy/mode" hx-target="#policy-screen" hx-swap="outerHTML" {
                    input type="hidden" name="scope" value=(scope.unwrap_or(""));
                    input type="hidden" name="revision" value=(revision);
                    // Four rows carry the same three controls, so each names
                    // its scope: a list of controls read aloud has to tell
                    // them apart.
                    select name="mode" aria-label=(format!("Mode for {named}")) {
                        @for option in ["observe", "prefer", "strict"] {
                            option value=(option) selected[option == mode] { (option) }
                        }
                    }
                    button type="button" class="btn quiet" aria-label=(format!("Forecast the mode for {named} over the record"))
                           hx-get="/app/policy/forecast" hx-include="closest form" hx-vals=r#"{"action":"mode"}"#
                           hx-target="#forecast-mode" hx-swap="innerHTML" { "Forecast" }
                    button type="submit" class="btn quiet" aria-label=(format!("Save the mode for {named}")) { "Save" }
                    @if inherited { span class="muted-inline" { "inherited" } }
                }
            }
            td {
                @match m.get("governing_engagement").and_then(Value::as_str) {
                    Some(engagement) => (engagement),
                    None => "—",
                }
            }
            td class="mono" { (u(m.get("distinct_cwds"))) }
            td class="mono" {
                @if denied.is_empty() { "—" } @else { (denied.join(", ")) }
                @if inherits_constraints { " " span class="muted-inline" { "inherited" } }
            }
            td class="mono" {
                @if access.is_empty() { "—" } @else {
                    @for rule in &access {
                        div {
                            (u(rule.get("position"))) ". " (str_of(rule.get("host"), "")) " → " (str_of(rule.get("action"), ""))
                            @if let Some(licence) = rule.get("licence").and_then(Value::as_str) { " " (licence) }
                        }
                    }
                }
            }
            // The digest of the effective policy a session in this scope
            // resolves to, the same one its mediated crossings record. Drift
            // evidence between edges, not proof of enforcement.
            td class="mono" {
                @match m.get("policy_identity") {
                    Some(Value::String(digest)) => (short_digest(digest)),
                    Some(Value::Object(shadow)) => {
                        "governed by " (str_of(shadow.get("shadowed_by"), "an earlier scope"))
                    }
                    _ => "—",
                }
            }
        }
    }
}

/// One scope where the governing and reported engagement disagree.
fn divergence_row(d: &Value) -> Markup {
    html! {
        tr {
            td class="mono" {
                @match d.get("scope").and_then(Value::as_str) {
                    Some(scope) => (scope),
                    None => "everything else",
                }
            }
            td { (str_of(d.get("governing"), "—")) }
            td { (str_of(d.get("reported"), "unattributed")) }
            td {
                @if d.get("allow_telemetry_egress") == Some(&Value::Bool(true)) { "cleared under the governing name" } @else { "not cleared" }
            }
        }
    }
}

/// The attribution rules editor: ordered rows, first match wins, one empty row
/// to add with. A row whose match is cleared is dropped on save. Attribution
/// changes what recorded work is reported under and nothing else.
fn attribution_card(rules: &Value) -> Markup {
    let revision = rules.get("revision").and_then(Value::as_str);
    let listed: Vec<&Value> = rules
        .get("rules")
        .and_then(Value::as_array)
        .map(|list| list.iter().collect())
        .unwrap_or_default();
    let error = rules.get("error").and_then(Value::as_str);
    html! {
        div class="card" {
            h2 { "Attribution rules" span class="muted-inline" { "which engagement recorded work is reported under" } }
            @if let Some(error) = error {
                p class="muted" { "The rule file did not load: " (error) " Saving replaces it." }
            }
            @match revision {
                Some(revision) => {
                    form class="rules-form" method="post" action="/app/attribution"
                         hx-post="/app/attribution" hx-target="#policy-screen" hx-swap="outerHTML" {
                        input type="hidden" name="revision" value=(revision);
                        div class="rule-head" aria-hidden="true" { span { "Directory contains" } span { "Reported engagement" } }
                        @for (index, rule) in listed.iter().enumerate() {
                            div class="rule-row" {
                                input type="text" name="match" class="mono" value=(str_of(rule.get("match"), ""))
                                    aria-label=(format!("Directory contains, rule {}", index + 1));
                                input type="text" name="engagement" value=(str_of(rule.get("engagement"), ""))
                                    aria-label=(format!("Reported engagement, rule {}", index + 1));
                            }
                        }
                        div class="rule-row" {
                            input type="text" name="match" class="mono" placeholder="code/ozone"
                                aria-label="Directory contains, new rule";
                            input type="text" name="engagement" placeholder="ozone"
                                aria-label="Reported engagement, new rule";
                        }
                        div class="rule-actions" {
                            button type="submit" class="btn" { "Save rules" }
                        }
                    }
                }
                None => p class="muted" { "The rule file could not be read, so it cannot be edited here." }
            }
            p class="callout" { "Rules are read in order and the first whose text appears in a session's working directory names its reported engagement; work no rule names is reported as unattributed. Saving changes what recorded work is reported under; it changes no evidence and enforces nothing." }
        }
    }
}

/// The dry run's answer, rendered under the form that asked for it. Each grade
/// is its own line and none is totalled with another; the examples carry the
/// runtime's own reason.
pub fn forecast_fragment(forecast: &Value, draft: &Value) -> String {
    let markup = html! {
        @if let Some(error) = forecast.get("error").and_then(Value::as_str) {
            p class="muted" { span class="stop" { "No forecast: " } (error) }
        } @else {
            p class="muted" {
                "If " (str_of(draft.get("description"), "this draft")) " had been in force, over the "
                (u(forecast.get("crossings"))) " crossings on record. Nothing is saved by a forecast."
            }
            p class="callout" {
                "Judged as principal " (str_of(forecast["principal"].get("name"), "unknown"))
                " (" (str_of(forecast["principal"].get("basis"), "unknown")) "), the principal this console runs as; "
                (u(forecast["principal"].get("other_principal_crossings")))
                " of these crossings were recorded under another principal and are judged under this one's overlay, not their own. "
                "Reconstructed crossings carry no working directory and are judged under the top-level policy. "
                (u(forecast.get("not_evaluated"))) " crossing records carry no URL and could not be judged."
            }
            div class="table-wrap" {
                table class="rules" {
                    thead { tr {
                        th { "Grade" } th { "Crossings" } th { "Newly refused" } th { "Newly a breach" } th { "Newly admitted" } th { "Unchanged" }
                    } }
                    tbody {
                        @for grade in ["witnessed", "reconstructed", "refused"] {
                            @let g = &forecast["grades"][grade];
                            tr {
                                td { (grade) }
                                td class="mono" { (u(g.get("crossings"))) }
                                td class="mono" { (u(g.get("newly_refused"))) }
                                td class="mono" { (u(g.get("newly_breached"))) }
                                td class="mono" { (u(g.get("newly_admitted"))) }
                                td class="mono" { (u(g.get("unchanged"))) }
                            }
                        }
                    }
                }
            }
            @for grade in ["witnessed", "reconstructed", "refused"] {
                @let examples = forecast["grades"][grade]["examples"].as_array().cloned().unwrap_or_default();
                @if !examples.is_empty() {
                    h3 class="xh" { (grade) ", for example" }
                    @for example in &examples {
                        div class="row" {
                            span class="mono" { (str_of(example.get("url"), "")) }
                            span class="eng" {
                                (str_of(example.get("outcome"), ""))
                                @if let Some(scope) = example.get("scope").and_then(Value::as_str) { " in " (scope) }
                            }
                        }
                        @if let Some(reason) = example.get("reason").and_then(Value::as_str) {
                            p class="callout" { (reason) }
                        }
                    }
                }
            }
            @if let Some(json) = draft.get("json").and_then(Value::as_str) {
                p class="callout" { "To adopt it, add this to the scope's constraints in the policy file:" }
                pre class="mono" { code { (json) } }
            }
        }
    };
    markup.into_string()
}

/// A digest shortened for a table cell, with the full value on hover and in
/// the JSON projection.
fn short_digest(digest: &str) -> Markup {
    let shown: String = digest.chars().take(19).collect();
    html! { span title=(digest) { (shown) "…" } }
}

/// A revision token shortened for the provenance line.
fn short(revision: &str) -> String {
    revision.chars().take(12).collect()
}

/// The Sources screen: the providers the agent may pull content from, and
/// which hold a key. The connected flag is computed by the CLI at start from
/// the operator's own credentials — the console never holds a key.
fn sources(providers: &[Value]) -> Markup {
    html! {
        (topbar("Sources", html! {
            "Key presence was read from the operator's credentials when the console started."
        }))
        section class="screen" {
            @if providers.is_empty() {
                div class="card" { p class="muted" { "No content providers are configured." } }
            } @else {
                @for provider in providers { (source_row(provider)) }
                p class="callout" {
                    "A source tells you where content came from; it never asserts you have the right to use it. Licence stays unknown unless a provider states one."
                }
            }
        }
    }
}

fn source_row(provider: &Value) -> Markup {
    let name = str_of(provider.get("name"), "");
    let connected = provider
        .get("connected")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let avatar: String = name.chars().take(2).collect();
    html! {
        div class="src" data-provider=(name) {
            div class="src-avatar" aria-hidden="true" { (avatar) }
            div class="src-body" {
                div class="src-name" { (provider_title(name)) }
                div class="src-use" { (provider_description(name)) }
            }
            div class="src-status" {
                @if connected {
                    span class="badge b-grounded" { "connected" }
                } @else {
                    span class="badge b-plain" { "no key" }
                }
            }
        }
    }
}

fn provider_title(name: &str) -> &str {
    match name {
        "exa" => "Exa",
        "firecrawl" => "Firecrawl",
        "tavily" => "Tavily",
        "tollbit" => "TollBit",
        "parallel" => "Parallel",
        "linkup" => "Linkup",
        "search1api" => "Search1API",
        "serpdive" => "SERPdive",
        "keenable" => "Keenable",
        "you" => "You.com",
        "nimble" => "Nimble",
        "ozone" => "Ozone Live",
        "tinyfish" => "TinyFish",
        "redpine" => "Redpine",
        other => other,
    }
}

fn provider_description(name: &str) -> &str {
    match name {
        "exa" => "Web search and page contents",
        "firecrawl" => "Scrape a known URL to clean content",
        "parallel" => "Objective-led search and extract",
        "tollbit" => "Licensed publisher content",
        "tinyfish" => "Web search and fetch",
        "redpine" => "Licensed datasets and real-time signals",
        "ozone" => "Licensed publisher passages and full text",
        _ => "Web search",
    }
}

/// The Compare screen: run one query across every connected source and see
/// what each returns and at what cost. These are real, credit-spending calls
/// the operator triggers — a probe, not context the agent received — so the
/// screen says so before it runs anything. With no source holding a key the
/// screen says the query is unavailable and why, rather than reporting an
/// empty result nobody asked for (`docs/FAIL-POLICY.md` §5).
fn compare(query: Option<&str>, results: &[Value], providers: &[Value]) -> Markup {
    let connected = providers
        .iter()
        .filter(|p| p.get("connected").and_then(Value::as_bool) == Some(true))
        .count();
    html! {
        (topbar("Compare", html! {
            "One query, run live across every source holding a key; nothing is recorded."
        }))
        section class="screen" {
            form class="compare-form" method="post" action="/app/compare" {
                label class="sr-only" for="compare-query" { "Query" }
                input class="compare-q" type="text" name="query" id="compare-query"
                    value=(query.unwrap_or("")) placeholder="a query to run across your sources";
                button class="btn" type="submit" { "Run" }
            }
            @if connected == 0 {
                p class="muted" role="status" {
                    span class="stop" { "Unavailable: " }
                    "no source holds a key, so no query is sent. Keys are read from the credentials file when the console starts; "
                    a href="/app/sources" { "Sources" } " lists each provider."
                }
            } @else {
                @match query {
                    None => p class="muted" {
                        (connected) " of " (providers.len()) " sources hold a key. A run makes real calls and spends real credits — it is a probe you trigger, not something your agent runs."
                    },
                    Some(q) if results.is_empty() => p class="muted" role="status" {
                        "No connected source returned anything for “" (q) "”."
                    },
                    Some(_) => {
                        p class="callout" {
                            "Each connected source ran the same query. Costs are in each provider's own units — a source that discloses no price shows unknown, never zero."
                        }
                        div class="compare-grid" {
                            @for result in results { (provider_result(result)) }
                        }
                    }
                }
            }
        }
    }
}

fn provider_result(result: &Value) -> Markup {
    let name = str_of(result.get("provider"), "");
    let error = result.get("error").and_then(Value::as_str);
    let items = result
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    html! {
        div class="cmp-col" {
            div class="cmp-head" {
                h2 class="cmp-name" { (provider_title(name)) }
                @match error {
                    Some(_) => span class="badge b-plain" { "unavailable" },
                    None => span class="cmp-count" { (u(result.get("results_count"))) " results" },
                }
            }
            @match error {
                Some(reason) => p class="cmp-err" { (reason) },
                None => {
                    div class="cmp-facts" {
                        span { "cost " (cost_str(result.get("cost"))) }
                        span { "latency " (latency_str(result.get("latency_ms"))) }
                    }
                    div class="cmp-results" {
                        @for item in items.iter().take(5) {
                            div class="cmp-item" {
                                span class="cmp-item-host" { (str_of(item.get("host"), "")) }
                                span class="cmp-item-title" {
                                    (str_of(item.get("title"), str_of(item.get("url"), "")))
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A charge in the provider's own unit, or "unknown". Currency and native
/// units are never merged; a quoted price says so.
fn cost_str(cost: Option<&Value>) -> String {
    let Some(cost) = cost.filter(|c| !c.is_null()) else {
        return "unknown".to_owned();
    };
    if let Some(money) = cost.get("money").filter(|m| !m.is_null()) {
        let currency = str_of(money.get("currency"), "");
        let micros = money.get("micros").and_then(Value::as_u64).unwrap_or(0);
        return format!("{currency} {:.6}", micros as f64 / 1e6);
    }
    if let Some(native) = cost.get("native").filter(|n| !n.is_null()) {
        let amount = native
            .get("amount")
            .map(std::string::ToString::to_string)
            .unwrap_or_default();
        let unit = str_of(native.get("unit"), "");
        let quoted = if native.get("basis").and_then(Value::as_str) == Some("quoted") {
            " (quoted)"
        } else {
            ""
        };
        return format!("{amount} {unit}{quoted}");
    }
    "unknown".to_owned()
}

fn latency_str(value: Option<&Value>) -> String {
    match value.and_then(Value::as_u64) {
        Some(ms) => format!("{ms} ms"),
        None => "unknown".to_owned(),
    }
}

// ---- small presentational helpers ----

fn metric(n: &str, k: &str, kind: &str) -> Markup {
    html! {
        div class="metric" {
            div class=(format!("n {kind}")) { (n) }
            div class="k" { (k) }
        }
    }
}

/// A crossing's host and path, styled the same wherever a crossing is shown.
fn host_cell(row: &Value) -> Markup {
    let url = row.get("url").and_then(Value::as_str).unwrap_or("");
    let host = row.get("host").and_then(Value::as_str).unwrap_or("");
    let path = url
        .split_once("://")
        .map(|(_, rest)| rest.trim_start_matches(host))
        .unwrap_or(url);
    html! {
        span class="host" { (host) }
        span class="path" { (path) }
    }
}

fn engagements_of(row: &Value) -> String {
    row.get("engagements")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn str_of<'a>(value: Option<&'a Value>, fallback: &'a str) -> &'a str {
    value.and_then(Value::as_str).unwrap_or(fallback)
}

fn u(value: Option<&Value>) -> u64 {
    value.and_then(Value::as_u64).unwrap_or(0)
}

/// An RFC 3339 timestamp cut to minute precision; unknown stays the word.
fn when(value: Option<&Value>) -> String {
    match value.and_then(Value::as_str) {
        Some(ts) => ts.replace('T', " ").chars().take(16).collect(),
        None => "unknown".to_owned(),
    }
}

/// One fact cell: a small key over its value; `stop` colours the value as a
/// refusal count.
fn fact(k: &str, v: &str, stop: bool) -> Markup {
    html! {
        div class="fact" {
            span class="k" { (k) }
            span class=(if stop { "v stop" } else { "v" }) { (v) }
        }
    }
}

/// A crossing's grade — observed, mediated or reconstructed — read from the
/// event name, which is the record's own word for how it was seen.
fn grade_of(record: &Value) -> &str {
    record
        .get("event")
        .and_then(Value::as_str)
        .and_then(|event| event.strip_prefix("crossing_"))
        .unwrap_or("observed")
}

fn grade_badge_class(grade: &str) -> &'static str {
    match grade {
        "mediated" => "badge b-mediated",
        "reconstructed" => "badge b-recon",
        _ => "badge b-observed",
    }
}

/// The reason a crossing was refused, if it was — a mediated crossing this
/// runtime declined before it reached the agent.
fn refusal_of(record: &Value) -> Option<&str> {
    record
        .get("payload")
        .and_then(|payload| payload.get("refusal"))
        .and_then(Value::as_str)
}

/// The origin's own refusal of a request, if it made one. A different fact
/// from `refusal_of`, which is this operator's policy.
fn challenge_of(record: &Value) -> Option<&str> {
    record
        .get("payload")
        .and_then(|payload| payload.get("challenge"))
        .and_then(Value::as_str)
}

/// What the request presented, in one phrase: the enrolled key it was signed
/// with, or that it carried no signature and the recorded reason. Absent where
/// no request left the machine, which is every crossing but a mediated one.
fn identity_of(payload: &Value) -> Option<String> {
    let identity = payload.get("identity")?;
    match identity.get("key_id").and_then(Value::as_str) {
        Some(key_id) => Some(format!("signed as {key_id}")),
        None => Some(match identity.get("unsigned").and_then(Value::as_str) {
            Some(reason) => format!("unsigned — {reason}"),
            None => "unsigned".to_owned(),
        }),
    }
}

fn host_of(url: &str) -> String {
    url.split_once("://")
        .map(|(_, rest)| rest.split('/').next().unwrap_or(""))
        .unwrap_or("")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const READ: ReadFrom<'static> = ReadFrom {
        sessions: "/home/op/.commonmeasure/sessions",
        at: "2026-09-13 09:00 UTC",
    };

    fn status() -> Value {
        json!({
            "records": 266,
            "engagements": [
                {"engagement": "ozone", "witnessed": 79, "reconstructed": 0, "refused": 2, "sessions": 11},
                {"engagement": "commonmeasure", "witnessed": 151, "reconstructed": 0, "refused": 0, "sessions": 10}
            ],
            "egress": {"receiver": "https://hub.example/api/v1/telemetry", "delivered": 4, "pending": 0,
                       "key_id": "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k", "key_standing": "enrolled"}
        })
    }

    #[test]
    fn overview_totals_across_engagements_and_never_reads_a_missing_count_as_zero() {
        let page = overview_page(&status(), &json!([]), READ);
        // 79 + 151 witnessed across the two engagements.
        assert!(page.contains(">230</div>"));
        // Refused total surfaced, and the active section marked in the nav.
        assert!(page.contains("refused by your policy"));
        assert!(page.contains("nav-item on"));
        assert!(page.contains(r#"aria-current="page""#));
        // The cleared-to-hub total is a metric on the Overview, and the
        // hub card names the receiver and the enrolled key with its standing.
        assert!(page.contains("cleared to the hub"));
        assert!(page.contains("https://hub.example/api/v1/telemetry"));
        assert!(page.contains("kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k"));
        assert!(page.contains("enrolled."));
        // The screen says what it read and when, and has one level-one heading.
        assert!(page.contains("/home/op/.commonmeasure/sessions"));
        assert!(page.contains("2026-09-13 09:00 UTC"));
        assert_eq!(page.matches("<h1>").count(), 1);

        let mut unenrolled = status();
        unenrolled["egress"] = json!({"receiver": null, "delivered": 0, "pending": 0,
                                      "key_id": null, "key_standing": null});
        let page = overview_page(&unenrolled, &json!([]), READ);
        assert!(page.contains("Not enrolled with a hub"));
    }

    /// The sidebar names the product, and an error answers as a page in the
    /// shell rather than a bare body.
    #[test]
    fn the_shell_names_the_product_and_errors_render_inside_it() {
        let page = sources_page(&[]);
        assert!(page.contains(r#"<div class="wordmark">Common Measure</div>"#));
        assert!(
            !page.contains("context"),
            "the retired name is gone: {page}"
        );
        assert!(page.contains(r#"lang="en-GB""#));
        assert!(page.contains(r##"href="#main""##));
        let error = error_page(404, "no such route");
        assert!(error.contains("<h1>404 Not found</h1>"), "{error}");
        assert!(error.contains("no such route"));
        assert!(error.contains(r#"class="app""#));
    }

    #[test]
    fn compare_prompts_before_a_query_and_shows_costed_results_after() {
        let providers = vec![
            json!({"name": "exa", "connected": true}),
            json!({"name": "tollbit", "connected": false}),
        ];
        let empty = compare_page(None, &[], &providers);
        assert!(empty.contains("spends real credits"));
        assert!(empty.contains("1 of 2 sources hold a key"));
        assert!(empty.contains("class=\"app\""));
        assert!(
            empty.contains(r#"<label class="sr-only" for="compare-query">Query</label>"#),
            "the query input is labelled"
        );

        let results = vec![
            json!({"provider": "exa", "results_count": 3,
                   "cost": {"money": {"currency": "USD", "micros": 7000}}, "latency_ms": 240,
                   "results": [{"host": "www.gov.uk", "title": "Energy price cap",
                                "url": "https://www.gov.uk/x"}]}),
            json!({"provider": "tollbit", "error": "no key"}),
        ];
        let page = compare_page(Some("energy price cap"), &results, &providers);
        assert!(page.contains("Exa"));
        assert!(page.contains("USD 0.007000"));
        assert!(page.contains("www.gov.uk"));
        assert!(page.contains("unavailable")); // the errored provider is marked
    }

    /// With no source holding a key the screen names the missing dependency
    /// rather than reporting an empty result for a query it never sent.
    #[test]
    fn compare_with_no_key_anywhere_is_unavailable_and_says_why() {
        let providers = vec![json!({"name": "exa", "connected": false})];
        let page = compare_page(Some("energy price cap"), &[], &providers);
        assert!(page.contains("Unavailable: "), "{page}");
        assert!(page.contains("no source holds a key, so no query is sent"));
        assert!(!page.contains("returned anything"));
    }

    #[test]
    fn policy_shows_the_mode_and_scopes_when_declared_and_says_so_when_not() {
        let declared = json!({
            "declared": true, "source": "/home/op/.commonmeasure/policy.json",
            "revision": "0123456789abcdef0123",
            "modes": [
                {"scope": "~/work/ozone", "mode": "strict", "declares_mode": true,
                 "governing_engagement": "ozone", "distinct_cwds": 3},
                {"scope": Value::Null, "mode": "observe", "declares_mode": true, "distinct_cwds": 1}
            ],
            "engagements": [
                {"engagement": "ozone", "stances": [{"allow_telemetry_egress": true}]},
                {"engagement": "spur", "stances": [{"allow_telemetry_egress": false}]}
            ]
        });
        let rules =
            json!({"revision": "abc", "rules": [{"match": "code/ozone", "engagement": "ozone"}]});
        let page = policy_page(&declared, &rules, None, READ.at);
        assert!(page.contains(r#"value="observe" checked"#)); // the active mode radio
        assert!(page.contains("~/work/ozone"));
        assert!(page.contains("everything else")); // the top-level scope
        assert!(page.contains("1 clearing to the hub"));
        assert!(page.contains("Every scope's governing engagement"));
        assert!(
            page.contains(r#"name="revision" value="abc""#),
            "the rules form states its revision"
        );
        // The screen states the file it read, its revision and the time.
        assert!(page.contains("/home/op/.commonmeasure/policy.json"));
        assert!(page.contains("revision <span class=\"mono\">0123456789ab</span>"));
        assert!(page.contains("2026-09-13 09:00 UTC"));
        // Every rule input is labelled, and the per-scope controls are named
        // for their scope.
        assert!(page.contains(r#"aria-label="Directory contains, rule 1""#));
        assert!(page.contains(r#"aria-label="Reported engagement, rule 1""#));
        assert!(page.contains(r#"aria-label="Directory contains, new rule""#));
        assert!(page.contains(r#"aria-label="Mode for ~/work/ozone""#));
        assert!(page.contains(r#"aria-label="Save the mode for everything else""#));

        let undeclared =
            json!({"declared": false, "source": "/home/op/.commonmeasure/policy.json"});
        let page = policy_page(&undeclared, &rules, None, READ.at);
        assert!(page.contains("No policy is declared"));
        assert!(page.contains("never creates a policy file"));
        assert!(
            !page.contains("/app/policy/mode"),
            "no policy, no policy form"
        );
        assert!(
            page.contains("/app/attribution"),
            "attribution is editable without a policy"
        );
    }

    /// The scopes where the governing engagement (policy) and the reported
    /// engagement (attribution) disagree are listed with both names, and
    /// every form states the revision it was rendered from.
    #[test]
    fn policy_lists_each_engagement_disagreement_and_every_form_carries_the_revision() {
        let declared = json!({
            "declared": true, "source": "/home/op/.commonmeasure/policy.json",
            "revision": "f00d",
            "modes": [
                {"scope": "code/client", "mode": "strict", "declares_mode": true,
                 "declares_constraints": false, "denied_hosts": ["tracker.example"],
                 "access_rules": [{"position": 2, "host": "*.example.com", "action": "refuse"}],
                 "governing_engagement": "client-a", "distinct_cwds": 2},
                {"scope": Value::Null, "mode": "observe", "declares_mode": true,
                 "declares_constraints": true, "denied_hosts": ["tracker.example"], "access_rules": []}
            ],
            "engagements": [],
            "engagement_divergences": [
                {"scope": "code/client", "governing": "client-a", "reported": "client-b", "allow_telemetry_egress": true}
            ]
        });
        let rules = json!({"revision": "absent", "rules": []});
        let page = policy_page(
            &declared,
            &rules,
            Some(("saved", "Saved: something.")),
            READ.at,
        );
        assert!(page.contains(r#"id="divergences""#));
        assert!(page.contains("client-a") && page.contains("client-b"));
        assert!(page.contains("cleared under the governing name"));
        assert!(page.contains(r#"data-outcome="saved""#));
        assert!(page.contains("Saved: something."));
        assert_eq!(
            page.matches(r#"name="revision" value="f00d""#).count(),
            4,
            "the top-level dial, both scope rows and the block form state the policy revision"
        );
        assert!(page.contains(r#"name="revision" value="absent""#));
        assert!(page.contains("2. *.example.com → refuse"));
        assert!(page.contains("tracker.example"));
    }

    #[test]
    fn a_forecast_keeps_the_grades_apart_and_shows_the_json_to_adopt() {
        let forecast = json!({
            "saved": false, "crossings": 3,
            "grades": {
                "witnessed": {"crossings": 2, "unchanged": 1, "newly_refused": 1, "newly_breached": 0, "newly_admitted": 0,
                              "examples": [{"url": "https://cdn.tracker.example/a", "scope": "code/client", "outcome": "refused", "reason": "access rule 1 (*.tracker.example) refuses host cdn.tracker.example."}]},
                "reconstructed": {"crossings": 1, "unchanged": 0, "newly_refused": 1, "newly_breached": 0, "newly_admitted": 0, "examples": []},
                "refused": {"crossings": 0, "unchanged": 0, "newly_refused": 0, "newly_breached": 0, "newly_admitted": 0, "examples": []}
            }
        });
        let draft = json!({"description": "refuse *.tracker.example in scope \"code/client\"",
                           "json": "{\"kind\": \"access_rule\"}"});
        let fragment = forecast_fragment(&forecast, &draft);
        assert!(fragment.contains("Nothing is saved by a forecast"));
        assert!(fragment.contains("<td>witnessed</td>"));
        assert!(fragment.contains("<td>reconstructed</td>"));
        assert!(fragment.contains("access rule 1 (*.tracker.example)"));
        assert!(fragment.contains("add this to the scope"));
        assert!(fragment.contains("Judged as principal unknown"));
        assert!(fragment.contains("Reconstructed crossings carry no working directory"));
        assert!(fragment.contains("0 crossing records carry no URL"));
        assert!(
            !fragment.contains(" 3 refused"),
            "grades are never totalled"
        );

        let failed = forecast_fragment(&json!({"error": "no such scope"}), &draft);
        assert!(failed.contains("No forecast: "));
    }

    #[test]
    fn sources_show_connected_and_no_key_providers() {
        let providers = vec![
            json!({"name": "exa", "connected": true}),
            json!({"name": "tollbit", "connected": false}),
        ];
        let page = sources_page(&providers);
        assert!(page.contains("Exa"));
        assert!(page.contains("Web search and page contents"));
        assert!(page.contains("connected"));
        assert!(page.contains("TollBit"));
        assert!(page.contains("no key"));
        assert!(
            !page.contains("toggle"),
            "no switch is drawn beside a state that is not a control"
        );
    }

    #[test]
    fn the_record_rail_groups_by_engagement_and_links_each_session() {
        let sessions = json!([
            {"session_id": "a29a9839-x", "engagement": "ozone", "host": "claude-code",
             "observed": 13, "mediated": 2, "reconstructed": 0, "refused": 2,
             "last": "2026-09-01T14:07:00Z"}
        ]);
        let page = record_page(&sessions, None, READ);
        assert!(page.contains("md-rail"));
        assert!(page.contains(">ozone<"));
        assert!(page.contains("a29a9839-x"), "the id is shown whole");
        assert!(page.contains("15 crossed")); // 13 observed + 2 mediated
        assert!(page.contains("2 refused"));
        // No breach carried, so the rail does not say "0 breached".
        assert!(!page.contains("breached"));
        assert!(page.contains("/app/fragments/session/a29a9839-x"));
        assert!(page.contains("hx-target=\"#detail\""));
        // A plain link reaches the same session with no script.
        assert!(page.contains(r#"href="/app/record?session=a29a9839-x""#));
        assert!(page.contains("No session selected."));
        assert!(!page.contains("aria-current=\"true\""));
        // The rail says where a Claude Code session's mediated crossings go.
        assert!(page.contains("separate"));
        assert!(page.contains("local-…"));
    }

    /// A session the server rendered into the pane is marked in the rail and
    /// opens with a focusable heading, so the chosen session is announced.
    #[test]
    fn a_selected_session_is_marked_in_the_rail_and_headed_in_the_pane() {
        let sessions = json!([
            {"session_id": "a29a9839-x", "engagement": "ozone", "host": "claude-code",
             "observed": 1, "mediated": 0, "reconstructed": 0, "refused": 0,
             "last": "2026-09-01T14:07:00Z"}
        ]);
        let records = json!([
            {"event": "crossing_observed", "payload": {
                "url": "https://www.gov.uk/x", "host_name": "www.gov.uk",
                "grounded": true, "licence": {"state": "unknown"}}}
        ]);
        let page = record_page(&sessions, Some(("a29a9839-x", &records)), READ);
        assert!(page.contains(r#"aria-current="true""#), "{page}");
        assert!(
            page.contains(r#"<h2 class="detail-head" tabindex="-1" autofocus>Session <span class="mono id">a29a9839-x</span></h2>"#),
            "{page}"
        );
        assert!(!page.contains("No session selected."));
        assert!(page.contains("https://www.gov.uk/x"));
    }

    /// The rail's "crossed" is observed plus mediated, the two counts
    /// the store serves per session. A session the mediator carried entirely
    /// — observed 0 — still crossed every one of them.
    #[test]
    fn a_fully_mediated_session_counts_its_mediated_crossings_as_crossed() {
        let sessions = json!([
            {"session_id": "local-7f3c1a2b", "engagement": "commonmeasure", "host": "claude-code",
             "observed": 0, "mediated": 5, "reconstructed": 0, "refused": 0,
             "last": "2026-09-02T10:00:00Z"}
        ]);
        let page = record_page(&sessions, None, READ);
        assert!(page.contains("5 crossed"), "{page}");
        assert!(!page.contains("0 crossed"), "{page}");
    }

    /// A session with no crossing is not an empty session. The pane says so
    /// where the label alone would read as "nothing happened": the store
    /// counts a turn boundary and a line it could not read, and the pane names
    /// both beside the label rather than filing the session as empty.
    #[test]
    fn a_session_with_only_turns_and_unreadable_lines_says_what_it_holds() {
        let records = json!([
            {"event": "turn_started", "payload": {"privacy_level": "minimal"}},
            {"event": "turn_completed", "payload": {"privacy_level": "minimal"}},
            {"event": "unreadable", "raw": "{not json"}
        ]);
        let detail = session_detail("local-7f3c1a2b", &records);
        assert!(
            detail.contains("This session recorded no crossings."),
            "the label partitions on crossings and says only that: {detail}"
        );
        assert!(
            detail
                .contains("Also recorded: 2 turn boundaries, 1 line this console could not read."),
            "{detail}"
        );
    }

    /// A session that crossed nothing and recorded nothing else says only
    /// that it crossed nothing: there is no second line to write.
    #[test]
    fn a_session_holding_nothing_at_all_carries_no_second_line() {
        let detail = session_detail("local-7f3c1a2b", &json!([]));
        assert!(detail.contains("This session recorded no crossings."));
        assert!(!detail.contains("Also recorded"), "{detail}");
    }

    /// The store's per-session `breached` count reaches the rail, and
    /// the detail pane counts breaches by the store's own rule — a record
    /// whose `payload.breach` is a string — as a stop-fact.
    #[test]
    fn a_carried_breach_is_counted_on_the_rail_and_in_the_detail_facts() {
        let sessions = json!([
            {"session_id": "local-7f3c1a2b", "engagement": "commonmeasure", "host": "claude-code",
             "observed": 0, "mediated": 4, "reconstructed": 0, "refused": 1, "breached": 3,
             "last": "2026-09-02T10:00:00Z"}
        ]);
        let page = record_page(&sessions, None, READ);
        assert!(
            page.contains("4 crossed · 1 refused · 3 breached"),
            "{page}"
        );

        let records = json!([
            {"event": "crossing_mediated", "payload": {
                "url": "https://www.gov.uk/energy-price-cap", "host_name": "www.gov.uk",
                "grounded": true, "licence": {"state": "unknown"}}},
            {"event": "crossing_mediated", "payload": {
                "url": "https://app.private-crm.io/export", "host_name": "app.private-crm.io",
                "breach": "host app.private-crm.io is denied; carried under prefer"}},
            {"event": "crossing_mediated", "payload": {
                "url": "https://example.org/x", "host_name": "example.org",
                "breach": null}}
        ]);
        let detail = session_detail("local-7f3c1a2b", &records);
        assert!(
            detail.contains("<span class=\"k\">breached</span><span class=\"v stop\">1</span>"),
            "{detail}"
        );
        // Without a breach the fact is present, neutral, and zero.
        let clean = session_detail("local-7f3c1a2b", &json!([records[0].clone()]));
        assert!(
            clean.contains("<span class=\"k\">breached</span><span class=\"v\">0</span>"),
            "{clean}"
        );
    }

    #[test]
    fn a_session_detail_shows_crossings_and_marks_a_refused_one() {
        let records = json!([
            {"event": "crossing_mediated", "payload": {
                "url": "https://www.gov.uk/energy-price-cap", "host_name": "www.gov.uk",
                "grounded": true, "licence": {"state": "unknown"}}},
            {"event": "crossing_mediated", "payload": {
                "url": "https://app.private-crm.io/export", "host_name": "app.private-crm.io",
                "refusal": "host not on your allow-list"}}
        ]);
        let detail = session_detail("a29a9839", &records);
        assert!(detail.contains("www.gov.uk/energy-price-cap"));
        assert!(detail.contains("b-grounded"));
        assert!(detail.contains("crossing refused"));
        assert!(detail.contains("host not on your allow-list"));
        // The witnessed crossing carries its grade; the refused one does not.
        assert!(detail.contains("b-mediated"));
    }

    /// Catches: the console showing a challenged fetch, a policy refusal and
    /// a transport failure as one outcome with no content, and a signed
    /// crossing as indistinguishable from an unsigned one. The session log
    /// keeps all four apart; a reader of this screen has to be able to as
    /// well (`docs/contracts/session-evidence.md`).
    #[test]
    fn a_session_detail_names_the_identity_presented_and_the_origins_own_refusal() {
        let records = json!([
            {"event": "crossing_mediated", "payload": {
                "url": "https://www.gov.uk/energy-price-cap", "host_name": "www.gov.uk",
                "grounded": true, "licence": {"state": "unknown"},
                "identity": {"user_agent": "CommonMeasureBot/0.2.0", "key_id": "key-1"}}},
            {"event": "crossing_mediated", "payload": {
                "url": "https://guarded.example/page", "host_name": "guarded.example",
                "http_status": 403, "grounded": false, "licence": {"state": "unknown"},
                "challenge": "the origin answered 403 with cf-mitigated: challenge",
                "identity": {"user_agent": "CommonMeasureBot/0.2.0",
                             "unsigned": "this edge is not enrolled with a hub"}}}
        ]);
        let detail = session_detail("a29a9839", &records);
        assert!(detail.contains("signed as key-1"), "{detail}");
        assert!(
            detail.contains("unsigned — this edge is not enrolled with a hub"),
            "{detail}"
        );
        assert!(detail.contains("challenged"), "{detail}");
        assert!(detail.contains("cf-mitigated: challenge"), "{detail}");
        assert!(
            detail.contains("<dt>status</dt><dd class=\"mono\">403</dd>"),
            "{detail}"
        );
        // Neither is a policy refusal: the operator allowed both.
        assert!(!detail.contains("crossing refused"), "{detail}");
    }

    /// The card shows the hashes and status the record carries and nothing
    /// the record lacks: a mediated fetch carries both hashes, the status and
    /// the identity; an observed crossing carries a content hash alone.
    #[test]
    fn a_crossing_card_shows_the_fields_the_record_carries_and_no_others() {
        let mediated = json!([{"event": "crossing_mediated", "payload": {
            "url": "http://127.0.0.1:9/index.html", "host_name": "127.0.0.1",
            "grounded": true, "licence": {"state": "unknown"}, "http_status": 200,
            "retrieved_hash": "sha256:aaaa", "content_hash": "sha256:bbbb",
            "identity": {"user_agent": "CommonMeasureBot/0.3.0", "unsigned": "no enrolment record"}}}]);
        let detail = session_detail("local-1", &mediated);
        assert!(
            detail.contains("<dt>bytes received</dt><dd class=\"mono\">sha256:aaaa</dd>"),
            "{detail}"
        );
        assert!(
            detail.contains("<dt>text read</dt><dd class=\"mono\">sha256:bbbb</dd>"),
            "{detail}"
        );
        assert!(
            detail.contains("<dt>status</dt><dd class=\"mono\">200</dd>"),
            "{detail}"
        );
        assert!(
            detail.contains("<dt>identity</dt><dd>unsigned — no enrolment record</dd>"),
            "{detail}"
        );

        let observed = json!([{"event": "crossing_observed", "payload": {
            "url": "https://www.gov.uk/x", "host_name": "www.gov.uk",
            "grounded": true, "licence": {"state": "unknown"}, "content_hash": "sha256:cccc"}}]);
        let detail = session_detail("s-1", &observed);
        assert!(
            detail.contains("<dt>text read</dt><dd class=\"mono\">sha256:cccc</dd>"),
            "{detail}"
        );
        assert!(!detail.contains("bytes received"), "{detail}");
        assert!(!detail.contains("<dt>status</dt>"), "{detail}");
        assert!(!detail.contains("<dt>identity</dt>"), "{detail}");
        assert!(
            !detail.contains("unknown</dd>"),
            "an absent field is absent, never unknown"
        );
    }
}
