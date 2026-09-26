//! The session evidence log: one file per harness session, appended to by
//! every hook process and by the MCP server.
//!
//! A run is a batch; a session is not. Hooks arrive as separate short-lived
//! processes over minutes or hours, so the log is opened for append rather than
//! created exclusively, and it reuses the run path's durability and gap
//! discipline (`commonmeasure_runtime::EvidenceLog`).

use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use commonmeasure_runtime::EvidenceLog;
use commonmeasure_types::Money;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[path = "observation_index.rs"]
mod observation_index;
#[path = "observations.rs"]
pub mod observations;

/// How a crossing came to be recorded. Never inferred from which code path
/// wrote the row: the observed path cannot refuse anything, and a record that
/// blurred the two would claim enforcement that did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossingMode {
    /// Seen after the fact through a lifecycle hook. Recorded, not governed.
    Observed,
    /// Carried by Common Measure itself, before the crossing, under policy.
    Mediated,
    /// Reconstructed from a host transcript after the fact, for work done
    /// before Common Measure was installed.
    ///
    /// The weakest of the three, and deliberately not called `observed`. A
    /// transcript is written by the host for its own purposes: it can be
    /// compacted or summarised, a tool result can be truncated in it, and
    /// nothing was watching at the time. A hash taken from one is a claim about
    /// the transcript, not about what entered the model's context.
    Reconstructed,
}

/// How the MCP client named itself in the protocol's `initialize` request:
/// its `clientInfo.name` and `clientInfo.version`, and `title` where it sent
/// one. `host` on a record is the registration's `--host` value, the word
/// the operator's configuration uses; this is the program's own word for
/// itself, which is what tells the Codex app from the Codex CLI, or Claude
/// Desktop's chat from its local agent mode, when both are registered the
/// same way. Recorded once per server process, and stamped on each
/// mediated crossing so one record answers who made it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientIdentity {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// One thing the agent read, or was refused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Crossing {
    pub session_id: String,
    /// When the record was written: after the response, and after any probe
    /// the crossing made once the page answered. Never the send time.
    pub timestamp: DateTime<Utc>,
    /// When the mediated fetch handed its page request to the transport,
    /// after any `Crawl-delay` or back-off wait, taken at the send and not
    /// derived from `timestamp`. Taken before connect, TLS and the write, so
    /// present where the origin received nothing. Where a redirect was
    /// followed it is the request to `redirects[0].requested_url`; later
    /// hops are not recorded. Absent where no page request was handed over
    /// (a refusal or a stop before the request), on every other kind of
    /// crossing, and on records written before the field existed; a reader
    /// never presents `timestamp` as the send time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_at: Option<DateTime<Utc>>,
    pub mode: CrossingMode,
    /// `claude-code`, `codex`, or whichever host observed it.
    pub host: String,
    /// The MCP client's own name and version, on a mediated crossing whose
    /// client sent them in `initialize`; absent on observed and
    /// reconstructed crossings, which no MCP client made, and on a mediated
    /// crossing from a client that named itself to nobody.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<ClientIdentity>,
    /// The host tool that produced it, for an observed crossing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Set only when the crossing happened inside a subagent, which is the
    /// host's own discriminator rather than something inferred from the
    /// payload shape. Main-agent calls carry neither field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// The host's own identifier for the turn this crossing happened in, so
    /// a reader can group crossings under the turn that caused them without
    /// the turn's question ever entering the record. Read from the hook
    /// payload (Claude Code `prompt_id`, Codex `turn_id`) and present only
    /// on observed crossings: the MCP server is told nothing about turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    /// The working directory the session was in when this crossing happened.
    /// A fact about where the work ran, recorded because sessions cross
    /// worktrees; which engagement that directory belongs to is resolved at
    /// read time in the sink, never written here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub url: String,
    pub host_name: String,
    /// True when this URL matched a `record_internal_prefixes` entry at
    /// capture time. Evidence, not configuration: egress consults this
    /// recorded fact, so editing or removing a prefix later cannot make a
    /// crossing that was classified operator-record-only leave the machine.
    /// Serialised only when true, so its absence is false. The relay also
    /// checks the address floor and the current prefix list, which covers
    /// session logs recorded before the field existed: those stay evidence a
    /// sealed run may cite (`docs/contracts/session-evidence.md`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub internal: bool,
    /// Present when the bytes entered the model's context and this runtime saw
    /// them. Absent for a search result, where only a title and snippet did.
    /// On a mediated fetch it covers the whole text extracted from the body,
    /// delivered or withheld, which for an HTML page is the readable text and
    /// not the markup. A result carries at most a part of that text;
    /// `delivered` names the part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    /// SHA-256 over the response body as the origin served it, on a mediated
    /// fetch that received one: the coded bytes where the origin served the
    /// body under gzip. Equal to `content_hash` where the body was served
    /// with no content coding and delivered as decoded; where the two differ,
    /// the transform-stage
    /// invocation recorded before the crossing carries both and ties them.
    /// Never projected onto the Content Telemetry wire: what a receiver
    /// learns is `content_hash`, the hash of the whole extracted text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieved_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_tokens: Option<u64>,
    /// The part of the extracted text a mediated fetch handed the host, on a
    /// crossing that returned text. `content_hash` describes the whole
    /// extracted body and `estimated_tokens` this part; `delivered` names the
    /// slice the result carried. The edge knows what it handed the host, not
    /// what the host kept, stored or excerpted from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered: Option<Delivered>,
    /// True only for tools whose result put page text into context.
    pub grounded: bool,
    /// `unknown` unless a supplier declared a machine-readable licence. No
    /// open-web provider does.
    pub licence: commonmeasure_types::LicenceState,
    /// The policy scope that governed this crossing: the scope's `match`
    /// string, or absent when the top-level policy applied or nothing governed
    /// at all. Only the mediated path records one — a record enforced under a
    /// policy that cannot be named is enforcement nobody can audit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_scope: Option<String>,
    /// The identity policy resolved before admission, with the basis that
    /// established it. Present on every mediated crossing and only on those:
    /// the other two modes captured what had already happened, outside any
    /// enforcement path. The basis can be `unavailable` — a platform this
    /// runtime cannot read an identity from authenticates nobody, and the
    /// record says so rather than omitting the field and reading like a log
    /// written before it existed (`docs/contracts/session-evidence.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authentication_basis: Option<String>,
    /// Set when policy refused this crossing. Only a mediated crossing can
    /// carry one; an observed crossing has already happened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    /// Set when policy found a breach and the mode carried the crossing
    /// anyway. The mode decides what happens to a breach, never whether it is
    /// recorded (`docs/FAIL-POLICY.md`), so an observe-mode crossing of a
    /// denied host carries the breach here where a strict one would carry a
    /// refusal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breach: Option<String>,
    /// For a reconstructed crossing, the transcript it was read from, so a
    /// reader can go back to the source this was inferred from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_from: Option<String>,

    // Source declarations and the answer the origin gave. Mediated fetches
    // only; a search result, an observed crossing and a reconstructed one
    // carry none of these (`docs/contracts/session-evidence.md` §Source
    // declarations).
    /// The HTTP status the final URL answered with. Absent for a search
    /// result, for a transport failure and for a refusal before the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    /// Why no answer arrived when the request left the machine: a transport
    /// failure, in the client's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    /// What the source declared about AI use, with each statement's origin,
    /// and what governed the crossing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declarations: Option<crate::discovery::Declarations>,
    /// Who named the source of a mediated fetch: `user` when a prompt of
    /// this session carried the URL, `agent` when prompts were recorded and
    /// none did, `unknown` when no prompt of this session was recorded. A
    /// fact for the operator's policy; not a carve-out from any preference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub named_by: Option<crate::prompt::NamedBy>,
    /// The `seq` of the `manifest_resolved` record written for this
    /// crossing's host, in this log, so a reader can go from the crossing to
    /// what discovery established without searching by host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_record: Option<u64>,
    /// The `Content-Telemetry-ID` the fetch carried (standard section 7.2):
    /// a UUID sent as a request header so the owner can match the retrieval
    /// event the relay projects to its own logs. Sent only to a host where
    /// a manifest or a licence was discovered before the request, so it is
    /// absent on the first fetch of a new host and on every search result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_telemetry_id: Option<Uuid>,
    /// The supplier that served a mediated search result: the provider name
    /// as policy, plans and the router use it (`exa`, `ozone`, `internal`,
    /// `skill:<name>`). Present only on a search result, which a supplier
    /// delivered under its own contract; a fetch has none, because this
    /// runtime made the request itself, and an observed or reconstructed
    /// crossing has none, because the host's tool was not a supplier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supplier: Option<String>,
    /// The network identity this request presented: the user agent, and the
    /// enrolled key it was signed with or why it carried no signature
    /// (`crate::identity`). Present on every mediated crossing whose request
    /// left the machine, and on no other: a crossing refused before the
    /// request presented nothing, and a search result was fetched by a
    /// supplier under its own contract, not by this runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<crate::identity::PresentedIdentity>,
    /// Set where the origin refused the fetcher rather than the resource: a
    /// challenge, or a status that refuses the request itself. A distinct fact
    /// from `refusal`, which is this operator's policy, and from `failure`,
    /// which is the transport; all three leave a crossing with no hash, and a
    /// reader has to be able to tell whose decision it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub challenge: Option<String>,
    /// The allowance gate's account of a fetch whose licence quoted a price:
    /// the consultation, the reservation held before the request and its
    /// settlement against the receipt (`commonmeasure_runtime::allowance`).
    /// Absent where no licence quoted a price or the principal declares no
    /// allowance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowance: Option<Value>,
}

/// The slice of a fetched body one `context_fetch` result carried.
///
/// Offsets and counts are in Unicode scalar values of the extracted text, the
/// unit `content_range` in the tool result uses; a slice never splits one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivered {
    /// Characters of the extracted text before the slice.
    pub offset: u64,
    /// Characters in the slice.
    pub chars: u64,
    /// Characters in the whole extracted text.
    pub total_chars: u64,
    /// SHA-256 over the UTF-8 bytes of the slice, in the `sha256:` form.
    pub hash: String,
}

impl Crossing {
    /// The token estimate's basis travels with the number, so nobody mistakes
    /// it for a tokeniser's output.
    pub fn to_record(&self) -> Value {
        let mut record = serde_json::to_value(self).unwrap_or(Value::Null);
        if self.estimated_tokens.is_some() {
            record["token_basis"] = json!(crate::grounding::TOKEN_BASIS);
        }
        record
    }
}

/// What one session's records add up to, counted once.
///
/// Two surfaces answer "what is in this session": `commonmeasure session` reads
/// the log, and the console renders the index derived from it. They were
/// counting separately — and then the console stopped counting at all, which
/// left a reader scrolling thousands of rows with no ratio anywhere, their
/// impression set by whichever rows happened to sit at the top. One
/// summariser, over the records either surface already holds, so the two
/// accounts of one session cannot disagree and neither can go silent.
///
/// Every field is a count of records, so zero is a real answer here and not
/// an unknown: absence of a grade means the log holds none of it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionSummary {
    pub records: u64,
    pub observed: u64,
    pub mediated: u64,
    pub refused: u64,
    pub reconstructed: u64,
    /// Crossings whose page text entered the model's context, as witnessed by
    /// this runtime.
    pub grounded_witnessed: u64,
    /// The same claim made only by a host transcript, kept apart because
    /// nothing was watching at the time.
    pub grounded_reconstructed: u64,
    /// Explicit host assertions kept separate from the grounding counts.
    pub host_observed: HostObservationSummary,
    /// Acquisitions awaiting host context evidence are not unread URL mentions.
    pub host_required: u64,
}

impl SessionSummary {
    /// Witnessed crossings that were carried: observed and mediated, never
    /// the refused ones — nothing crossed there — and never the reconstructed
    /// ones, which are a weaker claim and are never totalled with these.
    pub fn carried_witnessed(&self) -> u64 {
        self.observed + self.mediated
    }

    /// Carried witnessed crossings whose text did *not* enter the context: a
    /// URL was named and its page was not read. Search results are the bulk of
    /// it, and it is the half that makes the grounded count legible — a
    /// session of a thousand crossings and four fetches reads as neither
    /// number alone.
    pub fn named_not_read(&self) -> u64 {
        self.carried_witnessed()
            .saturating_sub(self.grounded_witnessed)
            .saturating_sub(self.host_required)
    }
}

/// Count one session's records: see [`SessionSummary`].
///
/// Reconstructed crossings are counted apart from the rest, in the grades and
/// in the grounding alike. A grounded total that merged them would claim this
/// runtime saw page text enter context when only a transcript says so.
pub fn summarise<'a>(records: impl IntoIterator<Item = &'a Value>) -> SessionSummary {
    let records: Vec<_> = records.into_iter().collect();
    let mut summary = SessionSummary {
        host_observed: host_observations(records.iter().copied(), None),
        ..Default::default()
    };
    for record in records {
        summary.records += 1;
        let event = record["event"].as_str();
        match event {
            Some("crossing_observed") => summary.observed += 1,
            Some("crossing_mediated") => summary.mediated += 1,
            Some("crossing_refused") => summary.refused += 1,
            Some("crossing_reconstructed") => summary.reconstructed += 1,
            _ => {}
        }
        if event == Some("crossing_mediated")
            && record["payload"]["context_observation"] == "host_required"
        {
            summary.host_required += 1;
        }
        if record["payload"]["grounded"] == Value::Bool(true)
            && record["payload"]["context_observation"] != "host_required"
        {
            if event == Some("crossing_reconstructed") {
                summary.grounded_reconstructed += 1;
            } else {
                summary.grounded_witnessed += 1;
            }
        }
    }
    summary
}

/// Counts of durable host observations joined to admitted acquisition handles.
/// These are host assertions, never Common Measure-witnessed grounding or
/// semantic support. Zero counts describe the record, not unobserved activity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HostObservationSummary {
    pub acquisitions: u64,
    pub context_entries: u64,
    pub context_acquisitions: u64,
    pub outputs: u64,
    pub output_associations: u64,
    pub unresolved_associations: u64,
}

impl HostObservationSummary {
    pub fn has_records(self) -> bool {
        self.acquisitions > 0 || self.context_entries > 0 || self.outputs > 0
    }

    /// Add independent sessions or acquisition groups without merging grades.
    pub fn merge(&mut self, other: Self) {
        self.acquisitions += other.acquisitions;
        self.context_entries += other.context_entries;
        self.context_acquisitions += other.context_acquisitions;
        self.outputs += other.outputs;
        self.output_associations += other.output_associations;
        self.unresolved_associations += other.unresolved_associations;
    }

    /// Private display projection; absent output observations leave association
    /// counts unknown. No handles, source URLs or output hashes are exported.
    pub fn to_value(self) -> Value {
        json!({
            "observer": "host", "grade": "observed",
            "acquisitions": self.acquisitions,
            "context_entries": self.context_entries,
            "context_acquisitions": self.context_acquisitions,
            "outputs": self.outputs,
            "output_associations": (self.outputs > 0).then_some(self.output_associations),
            "unresolved_associations": (self.outputs > 0).then_some(self.unresolved_associations),
        })
    }

    /// Counts with their evidence grade for text and HTML displays.
    pub fn display(self) -> String {
        format!(
            "host-observed (grade: observed): {} context entries across {} acquisitions; {} output associations; {} output observations",
            self.context_entries,
            self.context_acquisitions,
            if self.outputs > 0 {
                self.output_associations.to_string()
            } else {
                "unknown".into()
            },
            self.outputs,
        )
    }
}

/// Join observations in log order within their session and host. A handle can
/// recur in later generations; each representation counts as an entry and each
/// output marker counts as an association only when that generation observed
/// the acquisition in context. Unknown or out-of-generation markers stay apart.
/// `acquisition` narrows the display to one crossing's handle.
pub fn host_observations<'a>(
    records: impl IntoIterator<Item = &'a Value>,
    acquisition: Option<&str>,
) -> HostObservationSummary {
    use std::collections::{HashMap, HashSet};
    let mut summary = HostObservationSummary::default();
    let mut admitted = HashMap::new();
    let mut contexts = HashSet::new();
    let mut entered = HashSet::new();
    for record in records {
        let p = &record["payload"];
        let session = p["session_id"].as_str().unwrap_or("");
        let host = p["host"].as_str().unwrap_or("");
        let handle = p["acquisition_id"].as_str();
        let valid_id = |v: &Value| v.as_str().is_some_and(|s| Uuid::parse_str(s).is_ok());
        if record["event"] == "crossing_mediated"
            && p["observer"] == "cm"
            && p["grade"] == "mediated"
            && p["context_observation"] == "host_required"
            && !p["refusal"].is_string()
            && !p["failure"].is_string()
            && valid_id(&p["acquisition_id"])
        {
            let handle = handle.unwrap();
            if acquisition.is_none_or(|wanted| wanted == handle)
                && admitted.insert((session, host, handle), ()).is_none()
            {
                summary.acquisitions += 1;
            }
        }
        if p["observer"] != "host" || p["grade"] != "observed" || !valid_id(&p["generation_id"]) {
            continue;
        }
        let generation = p["generation_id"].as_str().unwrap();
        if record["event"] == "context_entered"
            && p["representation_hash"]
                .as_str()
                .is_some_and(observations::valid_hash)
            && let Some(handle) = handle
            && admitted.contains_key(&(session, host, handle))
        {
            summary.context_entries += 1;
            contexts.insert((session, host, handle, generation));
            entered.insert((session, host, handle));
        }
        if record["event"] == "output_associated"
            && valid_id(&p["output_id"])
            && p["output_hash"]
                .as_str()
                .is_some_and(observations::valid_hash)
            && let Some(handles) = p["acquisition_ids"].as_array()
        {
            if acquisition
                .is_some_and(|handle| !contexts.contains(&(session, host, handle, generation)))
            {
                continue;
            }
            summary.outputs += 1;
            for handle in handles {
                if acquisition.is_some_and(|wanted| handle.as_str() != Some(wanted)) {
                    continue;
                }
                if handle
                    .as_str()
                    .is_some_and(|handle| contexts.contains(&(session, host, handle, generation)))
                {
                    summary.output_associations += 1;
                } else {
                    summary.unresolved_associations += 1;
                }
            }
        }
    }
    summary.context_acquisitions = entered.len() as u64;
    summary
}

/// The policy-identity fields a session boundary record carries
/// (`docs/contracts/session-evidence.md` §Policy identity).
///
/// A boundary is written by a hook, which enforces nothing, so what it can
/// record is the identity of the policy a mediated crossing in this
/// directory would meet, resolved from the same document by the same
/// resolver. A policy the hook could not read is recorded as that:
/// `policy_unavailable` naming the reason, never an identity of a
/// permissive default the operator did not write. A record carrying
/// neither field predates the field.
pub fn boundary_policy(policy: Result<&crate::policy::SessionPolicy, &String>) -> Value {
    match policy {
        Ok(policy) => json!({"policy_identity": policy.identity().digest}),
        Err(reason) => json!({"policy_unavailable": reason}),
    }
}

/// The privacy level every turn boundary declares, in the Content Telemetry
/// vocabulary (`schema/telemetry-session.v1.json` `PrivacyLevel`: `full`,
/// `summary`, `intent`, `minimal`). It is a constant rather than a policy
/// field because the record holds nothing the higher levels would carry: no
/// query text, no response text, no classified intent, no summary. A
/// declared level the record could not honour would be a false claim, so the
/// operator cannot raise it here; a projection may only lower what it carries.
pub const TURN_PRIVACY_LEVEL: &str = "minimal";

/// Why the level is what it is, carried on the record so a reader needs no
/// other authority.
pub const TURN_PRIVACY_BASIS: &str = "the boundary records the host's turn identifier, the \
     working directory and the transcript path; it holds no query text, response text, intent \
     or summary, so minimal is the only level it can declare";

/// Where session evidence lives: `$COMMONMEASURE_HOME`, or `~/.commonmeasure`.
pub fn home_dir() -> std::io::Result<PathBuf> {
    if let Ok(home) = std::env::var("COMMONMEASURE_HOME")
        && !home.trim().is_empty()
    {
        return Ok(PathBuf::from(home));
    }
    let user = std::env::var("HOME")
        .map_err(|_| std::io::Error::other("neither COMMONMEASURE_HOME nor HOME is set"))?;
    Ok(PathBuf::from(user).join(".commonmeasure"))
}

/// The longest session identifier accepted, well above a UUID.
const SESSION_ID_MAX: usize = 128;

/// A session identifier, if it is safe to name a file with: ASCII letters,
/// digits, `.`, `_` and `-`, one to 128 characters, not starting with `.`.
///
/// A hook payload, a page, the command line and the server's environment
/// each supply session identifiers, and the identifier becomes a file name
/// under the operator home. Anything but a plain name is refused rather than
/// cleaned: a path separator would put records outside the sessions
/// directory, and a cleaned identifier would put one session's records in
/// another session's log.
pub fn safe_session(raw: &str) -> Option<&str> {
    let plain = raw
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    (plain && !raw.is_empty() && raw.len() <= SESSION_ID_MAX && !raw.starts_with('.'))
        .then_some(raw)
}

fn session_path(home: &Path, session_id: &str) -> std::io::Result<PathBuf> {
    let session_id = safe_session(session_id).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("session {session_id:?} is not a plain identifier"),
        )
    })?;
    Ok(home.join("sessions").join(format!("{session_id}.ndjson")))
}

/// Whether a log holds a complete `observations_started` record. Every hook
/// process opens its session's log, and most sessions never opt in, so a line
/// is parsed only if its bytes hold the event name: `serde_json`, which
/// writes every record here, never escapes those characters. A partial last
/// line belongs to a writer still writing and is not a marker yet.
fn opted_in(log: std::fs::File) -> std::io::Result<bool> {
    use std::io::BufRead;
    const MARKER: &[u8] = b"observations_started";
    let mut reader = std::io::BufReader::new(log);
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 || !line.ends_with(b"\n") {
            return Ok(false);
        }
        if line.windows(MARKER.len()).any(|window| window == MARKER)
            && serde_json::from_slice::<Value>(&line)
                .is_ok_and(|record| record["event"] == "observations_started")
        {
            return Ok(true);
        }
    }
}

pub struct SessionLog {
    session_id: String,
    log: EvidenceLog,
    path: PathBuf,
    /// The operator home, where a registration for this session is retained
    /// (`crate::instance`).
    home: PathBuf,
    /// The digest of the effective policy the process writing this log
    /// enforces, stamped on every mediated crossing it records. Set by the
    /// mediating server, which resolves its policy once at start; a hook
    /// process enforces nothing and leaves it unset.
    policy_identity: Option<String>,
    host_observations: bool,
    last_acquisition: Option<Uuid>,
    /// What observation requests are validated against, loaded by the first
    /// one: a hook process opens a log and never needs it.
    validation: Option<observation_index::ValidationIndex>,
}

/// Where a reader of this session's prompt records stopped, and what it has
/// gathered: the byte offset read to, every URL hash seen, and whether any
/// prompt record has been seen at all.
#[derive(Debug, Default)]
pub struct PromptCursor {
    pub offset: u64,
    pub hashes: Vec<String>,
    pub seen_prompt: bool,
}

impl SessionLog {
    /// Open one session's log for append. An identifier that is not a plain
    /// name ([`safe_session`]) is refused with `InvalidInput` and nothing is
    /// created.
    pub fn open(home: &Path, session_id: &str) -> std::io::Result<Self> {
        Self::open_path(home, session_id, session_path(home, session_id)?)
    }

    /// Comparison evidence uses the session format but stays outside the
    /// sessions directory that the telemetry relay scans.
    pub(crate) fn open_comparison(home: &Path, session_id: &str) -> std::io::Result<Self> {
        let path = session_path(home, session_id)?;
        Self::open_path(
            home,
            session_id,
            home.join("comparisons").join(path.file_name().unwrap()),
        )
    }

    pub(crate) fn record_comparison(
        &mut self,
        event: &str,
        payload: Value,
    ) -> std::io::Result<u64> {
        self.append(event, payload)
    }

    fn open_path(home: &Path, session_id: &str, path: PathBuf) -> std::io::Result<Self> {
        // Hooks share this file with other writers. Discovery must not make a
        // partial or damaged record prevent subsequent evidence from appending.
        // Observation validation still reads strictly.
        let host_observations = match std::fs::File::open(&path) {
            Ok(file) => opted_in(file)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error),
        };
        Ok(Self {
            session_id: session_id.to_owned(),
            log: EvidenceLog::open_append(&path)?,
            path,
            home: home.to_owned(),
            policy_identity: None,
            host_observations,
            last_acquisition: None,
            validation: None,
        })
    }

    /// Declare the policy this log's writer enforces, so each mediated
    /// crossing records the identity of the policy that ruled on it
    /// (`docs/contracts/session-evidence.md` §Policy identity).
    pub fn set_policy_identity(&mut self, identity: &crate::policy::PolicyIdentity) {
        self.policy_identity = Some(identity.digest.clone());
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one record, carrying the `instance` reference when a
    /// registration is retained for this session
    /// (`docs/contracts/session-evidence.md` §Where). The retained record is
    /// read at each append because a session is written by many short-lived
    /// processes and may be registered, renewed or closed between two of its
    /// records; the revision stamped is the one held when the record is
    /// written. A session with no registration gets no member, and a
    /// retained record that cannot be read leaves the member out: absence
    /// claims nothing.
    fn append(&mut self, event: &str, mut payload: Value) -> std::io::Result<u64> {
        if let Ok(Some(retained)) =
            crate::instance::Retained::for_session(&self.home, &self.session_id)
            && let Some(members) = payload.as_object_mut()
        {
            members.insert("instance".to_owned(), json!(retained.reference()));
        }
        self.log.append(event, payload)
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn record_crossing(&mut self, crossing: &Crossing) -> std::io::Result<u64> {
        let event = match (crossing.mode, crossing.refusal.is_some()) {
            (CrossingMode::Mediated, true) => "crossing_refused",
            (CrossingMode::Mediated, false) => "crossing_mediated",
            (CrossingMode::Observed, _) => "crossing_observed",
            (CrossingMode::Reconstructed, _) => "crossing_reconstructed",
        };
        self.last_acquisition = None;
        let mut record = crossing.to_record();
        let acquisition = (self.host_observations
            && crossing.mode == CrossingMode::Mediated
            && crossing.refusal.is_none()
            && crossing.failure.is_none()
            && crossing.content_hash.is_some()
            && crossing
                .http_status
                .is_some_and(|status| (200..300).contains(&status)))
        .then(Uuid::new_v4);
        // Search deliveries have no HTTP status and no returned handle. They
        // retain their existing crossing semantics outside fetch observations.
        // An answer this edge could not record returned no page, so there is
        // nothing for the host to observe.
        if self.host_observations
            && crossing.mode == CrossingMode::Mediated
            && crossing.failure.is_none()
            && crossing.http_status.is_some()
        {
            record["context_observation"] = json!("host_required");
            record["grounded"] = json!(false);
        }
        if let Some(handle) = acquisition {
            record["acquisition_id"] = json!(handle);
            record["crossing_id"] = json!(Uuid::new_v4());
            record["observer"] = json!("cm");
            record["grade"] = json!("mediated");
        }
        // Only a mediated crossing met a policy, so only a mediated crossing
        // names the policy it met. The other two grades were captured outside
        // any enforcement path and carry no identity, exactly as they carry
        // no principal.
        if crossing.mode == CrossingMode::Mediated
            && let Some(identity) = &self.policy_identity
        {
            record["policy_identity"] = json!(identity);
        }
        let seq = self.append(event, record)?;
        self.last_acquisition = acquisition;
        Ok(seq)
    }

    /// The handle of the last durably recorded acquisition in explicit host
    /// observation mode. A failed append withholds content from that host.
    pub fn acquisition_handle(&self) -> Result<Option<Uuid>, String> {
        if self.host_observations && self.last_acquisition.is_none() {
            return Err("unavailable: acquisition evidence was not durably recorded".to_owned());
        }
        Ok(self.last_acquisition)
    }

    /// Work stopped because the session's registered instance could not be
    /// shown to hold authority (`docs/contracts/session-evidence.md`
    /// §Instance registration): `outcome` is `refused` where the authority is
    /// known to have ended and `unavailable` where a required check could not
    /// be made. Nothing crossed, so this is never a crossing; the `instance`
    /// reference the append adds names the binding that was checked.
    pub fn record_instance_refusal(
        &mut self,
        host: &str,
        tool: &str,
        url: Option<&str>,
        stopped: &crate::instance::Stopped,
    ) -> std::io::Result<u64> {
        let mut payload = json!({
            "session_id": self.session_id,
            "host": host,
            "tool": tool,
            "outcome": stopped.outcome,
            "reason": stopped.reason,
        });
        if let Some(url) = url {
            payload["url"] = json!(url);
        }
        self.append("instance_refused", payload)
    }

    /// A provider policy breach before dispatch, without the query or a fabricated source.
    pub fn record_provider_ruling(
        &mut self,
        host: &str,
        provider: &str,
        ruling: &commonmeasure_runtime::policy::Ruling,
    ) -> std::io::Result<u64> {
        self.append(
            "provider_policy_ruled",
            json!({
                "session_id": self.session_id, "host": host, "tool": "context_search",
                "provider": provider,
                "outcome": if ruling.is_refusal() { "refused" } else { "allowed_with_breach" },
                "reason": ruling.reason(),
            }),
        )
    }

    /// One processor invocation, recorded beside the crossing it judged. The
    /// payload is the invocation record itself
    /// (`commonmeasure_runtime::processor::INVOCATION_VERSION`); the session log adds
    /// nothing and takes nothing away.
    pub fn record_processor(&mut self, invocation: Value) -> std::io::Result<u64> {
        self.append("processor_invoked", invocation)
    }

    /// A turn boundary. Cheap, and it is what lets a reader group crossings
    /// into the piece of work that caused them.
    ///
    /// `turn_id` is the host's own identifier for the turn, when it supplies
    /// one. The boundary declares [`TURN_PRIVACY_LEVEL`] because that is the
    /// only level whose claim the record can honour: it carries no question,
    /// no answer, no intent and no summary. `policy` is the boundary's
    /// policy-identity fields ([`boundary_policy`]), placed at the top level
    /// of the payload as on every other record that carries them, so one
    /// rule reads them all.
    pub fn record_turn(
        &mut self,
        event: &str,
        host: &str,
        turn_id: Option<&str>,
        detail: Value,
        policy: &Value,
    ) -> std::io::Result<u64> {
        let mut record = json!({
            "session_id": self.session_id,
            "host": host,
            "timestamp": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            "turn_id": turn_id,
            "privacy_level": TURN_PRIVACY_LEVEL,
            "privacy_basis": TURN_PRIVACY_BASIS,
            "detail": detail,
        });
        if let (Some(record), Some(fields)) = (record.as_object_mut(), policy.as_object()) {
            record.extend(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }
        self.append(event, record)
    }

    /// The standing mediation nudge, recorded at emission (`crate::nudge`).
    /// The record claims emission and nothing more: the host adds a
    /// `SessionStart` hook's stdout to the model's context, and that act is
    /// the host's, so the basis travels in the record. Whether the agent
    /// obeyed is not recorded either — that is what the session's own
    /// mediated-against-observed crossing counts measure.
    ///
    /// `policy` is the boundary's policy-identity fields
    /// ([`boundary_policy`]): the session starts under a named policy, or
    /// under one the hook could not read, and the record says which.
    pub fn record_nudge(
        &mut self,
        host: &str,
        source: Option<&str>,
        policy: &Value,
    ) -> std::io::Result<u64> {
        let mut record = json!({
            "session_id": self.session_id,
            "host": host,
            "timestamp": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            "nudge": crate::nudge::IDENTITY,
            "source": source,
            "basis": crate::nudge::BASIS,
        });
        if let (Some(record), Some(fields)) = (record.as_object_mut(), policy.as_object()) {
            record.extend(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }
        self.append("nudge_issued", record)
    }

    /// The identity this edge runs under, recorded at session start from the
    /// enrolment record: the hub by its origin alone, the key id the hub assigned, and the
    /// key's standing (`enrolled`, `revoked` with when and by which side, or
    /// `cleartext_hub` with `hub_refused` saying what is refused and the
    /// remedy). A session recorded after a revocation was learnt names the
    /// revoked key id and says so, so a reader can tell which sessions ran
    /// under a key publishers still honoured. Not written for an edge that is not
    /// enrolled: absence means no network identity, not an unknown one.
    ///
    /// The record also says whether the hub's key directory lists the key,
    /// as this edge last learnt it: `listed_until`, or `unlisted` with the
    /// reason. A signed request under a key the directory does not list
    /// verifies nowhere, so a standing key alone does not say that a
    /// publisher could verify this session's requests.
    ///
    /// `hub` is null where the stored URL has no origin to name (it does not
    /// parse, or its scheme has no host-based origin); `hub_refused` then
    /// says why nothing is sent to it. The stored URL itself is never written: 0.4.1's
    /// `connect` stored hub URLs with credentials in them.
    pub fn record_edge_identity(
        &mut self,
        host: &str,
        enrolment: &crate::enrolment::EnrolmentRecord,
        listing: &crate::enrolment::Listing,
    ) -> std::io::Result<u64> {
        let now = Utc::now();
        let mut record = json!({
            "session_id": self.session_id,
            "host": host,
            "timestamp": now.to_rfc3339_opts(SecondsFormat::Millis, true),
            "hub": crate::relay_config::receiver_origin(&enrolment.hub),
            "key_id": enrolment.key_id,
            "standing": enrolment.standing(),
            "revoked_at": enrolment.revoked_at,
            "revocation": enrolment.revocation,
        });
        if let Some(reason) = enrolment.hub_refused() {
            record["hub_refused"] = json!(reason);
        }
        match listing {
            crate::enrolment::Listing::ListedUntil(until) => {
                record["listed_until"] = json!(crate::enrolment::timestamp(*until));
            }
            crate::enrolment::Listing::Unlisted(reason) => {
                record["unlisted"] = json!(reason);
            }
        }
        self.append("edge_identity", record)
    }

    /// Whether this session's log already holds a record of `event`. Read
    /// before the log is opened for writing, so a server can tell whether a
    /// hook refreshed the policy for this session before it started.
    pub fn holds_event(home: &Path, session_id: &str, event: &str) -> bool {
        let Ok(path) = session_path(home, session_id) else {
            return false;
        };
        Self::read(&path)
            .map(|records| records.iter().any(|record| record["event"] == event))
            .unwrap_or(false)
    }

    /// One synchronisation of managed policy run for this session: at
    /// session start, so the session runs under the policy the hub
    /// currently desires, or the reason none could be attempted. `trigger`
    /// names what ran it (`session_start`, `server_start`). The payload is
    /// `commonmeasure_harness::managed::SyncReport::to_record`; an envelope
    /// that has expired is on it as `stale_since`
    /// (`docs/contracts/session-evidence.md` §Policy synchronisation).
    pub fn record_policy_sync(
        &mut self,
        host: &str,
        trigger: &str,
        sync: Value,
    ) -> std::io::Result<u64> {
        let mut record = json!({
            "session_id": self.session_id,
            "host": host,
            "timestamp": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            "trigger": trigger,
        });
        if let (Some(record), Some(fields)) = (record.as_object_mut(), sync.as_object()) {
            record.extend(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }
        self.append("policy_sync", record)
    }

    /// The host process this writer runs under, found by
    /// `crate::host_process::find`, so the hook log and the MCP log of one
    /// host session can be joined on it. `path` is `hook` or `mcp`. Written
    /// whether or not the walk found a process: where it did not,
    /// `host_process` is null and `unavailable` names why
    /// (`docs/contracts/session-evidence.md` §Host process).
    pub fn record_host_process(
        &mut self,
        host: &str,
        path: &str,
        finding: &crate::host_process::Finding,
    ) -> std::io::Result<u64> {
        let mut record = json!({
            "session_id": self.session_id,
            "host": host,
            "timestamp": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        });
        if let (Some(record), Some(fields)) =
            (record.as_object_mut(), finding.to_payload(path).as_object())
        {
            record.extend(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }
        self.log.append("host_process", record)
    }

    /// The host reported the session ending. `reason` is the host's own
    /// word, recorded as given and absent when the host sent none; a crash
    /// sends no hook, so a log without this record has not ended in any
    /// sense a reader may rely on (`docs/contracts/session-evidence.md`
    /// §Host process).
    pub fn record_session_ended(
        &mut self,
        host: &str,
        reason: Option<&str>,
    ) -> std::io::Result<u64> {
        let mut record = json!({
            "session_id": self.session_id,
            "host": host,
            "timestamp": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        });
        if let Some(reason) = reason {
            record["reason"] = json!(reason);
        }
        self.log.append("session_ended", record)
    }

    /// The operator's declared directory for a hosted service. This records
    /// the scope's basis without claiming a working directory on the client.
    pub fn record_hosted_scope(&mut self, host: &str, directory: &str) -> std::io::Result<u64> {
        self.append(
            "hosted_scope",
            json!({
                "session_id": self.session_id,
                "host": host,
                "timestamp": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                "basis": "service_configuration",
                "directory": directory,
            }),
        )
    }

    /// The MCP client's own name and version, as it sent them in the
    /// protocol's `initialize` request, with the protocol version it asked
    /// for and the one the server answered with. Written once, before the
    /// first crossing, so a session whose client named itself to nobody
    /// carries no such record and a reader is not handed a default name.
    pub fn record_client_identified(
        &mut self,
        host: &str,
        client: &ClientIdentity,
        protocol_version: Option<&str>,
        negotiated_protocol_version: Option<&str>,
    ) -> std::io::Result<u64> {
        let mut payload = json!({
            "session_id": self.session_id,
            "host": host,
            "timestamp": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            "client": client,
        });
        // Absent when the client named no protocol version, rather than a
        // null a reader would have to treat as a value.
        if let Some(protocol_version) = protocol_version {
            payload["protocol_version"] = json!(protocol_version);
        }
        // What the two ends agreed, which decides what the client could
        // send; it differs from the request where the client asked for a
        // revision the server does not serve.
        if let Some(negotiated) = negotiated_protocol_version {
            payload["negotiated_protocol_version"] = json!(negotiated);
        }
        self.append("client_identified", payload)
    }

    /// Where provider credentials came from at MCP server start: the
    /// operator file's path and digest plus variable names, or the fact that
    /// no file was present. The payload is built by
    /// `commonmeasure_supply::credentials::CredentialsStatus::to_value` and by
    /// construction carries no credential value; the log adds identity and
    /// the timestamp and takes nothing away.
    pub fn record_credentials(&mut self, host: &str, payload: Value) -> std::io::Result<u64> {
        self.append(
            "credentials_loaded",
            json!({
                "session_id": self.session_id,
                "host": host,
                "timestamp": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                "credentials": payload,
            }),
        )
    }

    /// The allowance ledger did not record what a mediated search did with
    /// the principal's allowance: a settlement it refused after one retry,
    /// or a release it refused. The payload names the reservation, the
    /// observed charge where the receipt carried one, and the reason, so the
    /// session states that money moved and the ledger did not count it — or
    /// that a reservation stays held — before the ledger's expiry sweep
    /// releases the reservation as a dead buyer's.
    pub fn record_allowance_gap(
        &mut self,
        host: &str,
        reservation_id: Option<Uuid>,
        observed: Option<&Money>,
        detail: &str,
    ) -> std::io::Result<u64> {
        self.append(
            "allowance_gap",
            json!({
                "session_id": self.session_id,
                "host": host,
                "timestamp": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                "reason": "evidence_missing",
                "reservation_id": reservation_id,
                "observed": observed,
                "detail": detail,
            }),
        )
    }

    /// What discovery established for a host after a mediated fetch
    /// (`crate::discovery::resolve_manifest`): the URL asked, the status,
    /// the cache decision and the manifest's facts or the rejection. Its own
    /// record kind, never a crossing, so a probe can never project as a
    /// retrieval. Returns the record's `seq` for the crossing to reference.
    pub fn record_manifest(
        &mut self,
        host: &str,
        resolution: &crate::discovery::ManifestResolution,
    ) -> std::io::Result<u64> {
        let mut payload = serde_json::to_value(resolution).unwrap_or(Value::Null);
        payload["session_id"] = json!(self.session_id);
        payload["host"] = json!(host);
        payload["timestamp"] = json!(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true));
        self.append("manifest_resolved", payload)
    }

    /// What one submitted prompt named and embedded (`crate::prompt`): URL
    /// hashes, never URL text, and the statements pasted material carried.
    pub fn record_prompt_sources(
        &mut self,
        host: &str,
        scan: &crate::prompt::PromptScan,
    ) -> std::io::Result<u64> {
        let mut payload = serde_json::to_value(scan).unwrap_or(Value::Null);
        payload["session_id"] = json!(self.session_id);
        payload["host"] = json!(host);
        payload["timestamp"] = json!(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true));
        self.append("prompt_sources", payload)
    }

    /// Read the `prompt_sources` records appended since `cursor` was last
    /// advanced and add their URL hashes to `hashes`. The hook and the MCP
    /// server are different processes sharing one log, so the server reads
    /// what the hook appended, from where it last stopped rather than from
    /// the start. Returns whether any prompt record has ever been seen, which
    /// is the difference between "the agent chose this URL" and "nothing can
    /// be said about who chose it".
    pub fn prompt_hashes_since(&self, cursor: &mut PromptCursor) -> bool {
        use std::io::{BufRead, Seek};
        let Ok(mut file) = std::fs::File::open(&self.path) else {
            return cursor.seen_prompt;
        };
        if file.seek(std::io::SeekFrom::Start(cursor.offset)).is_err() {
            return cursor.seen_prompt;
        }
        let mut reader = std::io::BufReader::new(file);
        let mut line = String::new();
        loop {
            line.clear();
            let Ok(read) = reader.read_line(&mut line) else {
                break;
            };
            if read == 0 || !line.ends_with('\n') {
                // A partial last line belongs to a writer still writing;
                // it is read whole next time.
                break;
            }
            cursor.offset += read as u64;
            let Ok(record) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if record["event"] != "prompt_sources" {
                continue;
            }
            cursor.seen_prompt = true;
            cursor.hashes.extend(
                record["payload"]["url_hashes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned),
            );
        }
        cursor.seen_prompt
    }

    /// A context-budget snapshot at a stable boundary. The payload is built
    /// by `crate::snapshot` and carries its own basis and unavailable list;
    /// the log adds the capture timestamp and takes nothing away.
    pub fn record_context_snapshot(&mut self, mut payload: Value) -> std::io::Result<u64> {
        payload["timestamp"] = json!(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true));
        self.append("context_snapshot", payload)
    }

    pub fn read(path: &Path) -> std::io::Result<Vec<Value>> {
        EvidenceLog::read(path)
    }

    /// Every session with a log, most recently modified first.
    pub fn list(home: &Path) -> std::io::Result<Vec<PathBuf>> {
        let directory = home.join("sessions");
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let mut sessions: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&directory)?
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "ndjson"))
            .filter_map(|entry| {
                let modified = entry.metadata().ok()?.modified().ok()?;
                Some((modified, entry.path()))
            })
            .collect();
        sessions.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
        Ok(sessions.into_iter().map(|(_, path)| path).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_observations_join_handles_and_generations_without_upgrading_grounding() {
        let acquisition = Uuid::new_v4();
        let generation = Uuid::new_v4();
        let output = Uuid::new_v4();
        let hash = format!("sha256:{}", "a".repeat(64));
        let records = vec![
            json!({"event":"crossing_mediated","payload":{"session_id":"s","host":"pi","observer":"cm","grade":"mediated","context_observation":"host_required","acquisition_id":acquisition}}),
            json!({"event":"context_entered","payload":{"session_id":"s","host":"pi","observer":"host","grade":"observed","acquisition_id":acquisition,"generation_id":generation,"representation_hash":hash}}),
            json!({"event":"output_associated","payload":{"session_id":"s","host":"pi","observer":"host","grade":"observed","generation_id":generation,"output_id":output,"output_hash":hash,"acquisition_ids":[acquisition,Uuid::new_v4()]}}),
            json!({"event":"crossing_mediated","payload":{"session_id":"other","host":"pi","observer":"cm","grade":"mediated","context_observation":"host_required","acquisition_id":Uuid::new_v4()}}),
        ];
        let summary = host_observations(&records, None);
        assert_eq!(summary.acquisitions, 2);
        assert_eq!(summary.context_entries, 1);
        assert_eq!(summary.context_acquisitions, 1);
        assert_eq!(summary.outputs, 1);
        assert_eq!(summary.output_associations, 1);
        assert_eq!(summary.unresolved_associations, 1);
        assert_eq!(summary.to_value()["grade"], "observed");
        assert!(summary.display().contains("host-observed"));
    }

    fn enrolled_at(hub: &str) -> crate::enrolment::EnrolmentRecord {
        serde_json::from_value(json!({
            "hub": hub,
            "organization": {"id": "org-1", "name": "Org"},
            "name": "laptop",
            "key_id": "key-1",
            "identity": {"origin": "https://hub.example", "bot_page": "https://hub.example/bot"},
            "enrolled_at": "2026-09-06T00:00:00.000Z",
        }))
        .unwrap()
    }

    /// The identity record as `record_edge_identity` wrote it, with the
    /// log's whole text.
    fn edge_identity_written(hub: &str) -> (Value, String) {
        let home = tempfile::tempdir().unwrap();
        let mut log = SessionLog::open(home.path(), "s").unwrap();
        log.record_edge_identity(
            "pi",
            &enrolled_at(hub),
            &crate::enrolment::Listing::Unlisted("no proof held".to_owned()),
        )
        .unwrap();
        let text = std::fs::read_to_string(log.path()).unwrap();
        let record = SessionLog::read(log.path())
            .unwrap()
            .into_iter()
            .find(|record| record["event"] == "edge_identity")
            .unwrap();
        (record["payload"].clone(), text)
    }

    /// 0.4.1's `connect` stored hub URLs with credentials in them; the
    /// session record names the hub by its origin alone.
    #[test]
    fn a_session_record_names_a_credentialed_hub_by_its_origin_alone() {
        let (payload, text) = edge_identity_written("https://user:ak_PLANTED@hub.example/");
        assert!(!text.contains("ak_PLANTED"), "{text}");
        assert_eq!(payload["hub"], "https://hub.example");
        assert_eq!(payload["standing"], "unusable_hub_url");
    }

    /// A stored hub with no origin to name is recorded as null, with
    /// `hub_refused` saying why nothing is sent to it; no part of the raw
    /// string is written.
    #[test]
    fn a_stored_hub_with_no_origin_is_recorded_as_null_beside_its_refusal() {
        for hub in [
            "https://ak_PLANTED@hub example.com/",
            "foo://ak_PLANTED@hub.example/",
            "ak_PLANTED",
        ] {
            let (payload, text) = edge_identity_written(hub);
            assert!(!text.contains("ak_PLANTED"), "{hub}: {text}");
            assert!(payload["hub"].is_null(), "{hub}: {payload}");
            assert_eq!(payload["standing"], "cleartext_hub", "{hub}");
            assert!(payload["hub_refused"].is_string(), "{hub}: {payload}");
        }
        // A scheme other than http and https that has an origin is named by it.
        let (payload, text) = edge_identity_written("ftp://ak_PLANTED@hub.example/");
        assert!(!text.contains("ak_PLANTED"), "{text}");
        assert_eq!(payload["hub"], "ftp://hub.example");
        assert_eq!(payload["standing"], "cleartext_hub");
    }

    fn crossing(event: &str, grounded: bool) -> Value {
        json!({"event": event, "payload": {"grounded": grounded}})
    }

    /// The counting rule both surfaces read: grades apart, and the
    /// transcript's grounding claim never added to the witnessed one.
    #[test]
    fn reconstructed_grounding_is_never_added_to_what_this_runtime_witnessed() {
        let records = vec![
            crossing("crossing_observed", true),
            crossing("crossing_observed", false),
            crossing("crossing_mediated", true),
            crossing("crossing_refused", false),
            crossing("crossing_reconstructed", true),
            crossing("crossing_reconstructed", true),
            json!({"event": "processor_invoked", "payload": {"decision": "admit"}}),
        ];
        let summary = summarise(&records);

        assert_eq!(
            summary.records, 7,
            "every record counts, not just crossings"
        );
        assert_eq!(summary.observed, 2);
        assert_eq!(summary.mediated, 1);
        assert_eq!(summary.refused, 1);
        assert_eq!(summary.reconstructed, 2);
        assert_eq!(summary.grounded_witnessed, 2);
        assert_eq!(summary.grounded_reconstructed, 2);
    }

    /// A refusal carried nothing, so it is not a crossing that named a page
    /// without reading it — counting it there would inflate the very figure
    /// the split exists to make honest.
    #[test]
    fn a_refusal_is_neither_carried_nor_counted_as_a_page_left_unread() {
        let summary = summarise(&vec![
            crossing("crossing_observed", true),
            crossing("crossing_observed", false),
            crossing("crossing_refused", false),
            crossing("crossing_refused", false),
        ]);
        assert_eq!(summary.carried_witnessed(), 2);
        assert_eq!(summary.named_not_read(), 1);
    }

    /// Only a plain name opens a log. A traversal, a separator, a leading dot,
    /// an empty or overlong identifier, or a character outside the set is
    /// refused, and nothing is created anywhere under the home or beside it.
    #[test]
    fn a_session_id_that_is_not_a_plain_name_opens_no_log() {
        let root = tempfile::tempdir().expect("tempdir");
        let home = root.path().join("home");
        for refused in [
            "../../x",
            "../x",
            "a/b",
            "a\\b",
            "..",
            ".hidden",
            "",
            "sp ace",
            "nul\0byte",
            "ünï",
            &"a".repeat(SESSION_ID_MAX + 1),
        ] {
            let error = SessionLog::open(&home, refused)
                .err()
                .unwrap_or_else(|| panic!("{refused:?} opened a log"));
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::InvalidInput,
                "{refused:?}"
            );
            assert!(safe_session(refused).is_none(), "{refused:?}");
            assert!(!SessionLog::holds_event(&home, refused, "policy_sync"));
        }
        assert_eq!(
            std::fs::read_dir(root.path()).unwrap().count(),
            0,
            "a refused identifier created something under or beside the home"
        );

        for accepted in [
            "s-1",
            "local-1789404537244-51182",
            "20260914_1",
            "8195e8c6-94a2-4f8d-b6ca-545246a50ea6",
            "a.b",
            &"a".repeat(SESSION_ID_MAX),
        ] {
            let mut log = SessionLog::open(&home, accepted).expect(accepted);
            log.record_context_snapshot(json!({})).expect("append");
            assert_eq!(
                log.path(),
                home.join("sessions").join(format!("{accepted}.ndjson"))
            );
        }
    }

    /// An empty log is zero of everything and never a panic or an underflow.
    #[test]
    fn an_empty_session_counts_to_zero() {
        let summary = summarise(&Vec::new());
        assert_eq!(summary, SessionSummary::default());
        assert_eq!(summary.named_not_read(), 0);
    }

    // Catches: a reader that requires the send time, or a writer that fills
    // it in, on a mediated crossing written by 0.4.2, which has none.
    #[test]
    fn a_crossing_written_before_the_send_time_reads_without_one() {
        let written = json!({
            "session_id": "s", "timestamp": "2026-09-20T10:00:00.250Z", "mode": "mediated",
            "host": "claude-code", "url": "https://publisher.example/page",
            "host_name": "publisher.example", "http_status": 200,
            "content_hash": format!("sha256:{}", "a".repeat(64)),
            "grounded": true, "licence": {"state": "unknown"},
            "principal": "os-user:test", "authentication_basis": "os_user"
        });
        let crossing: Crossing = serde_json::from_value(written.clone()).expect("0.4.2 reads");
        assert_eq!(crossing.requested_at, None);
        assert_eq!(crossing.to_record(), written);
        let summary = summarise(&[json!({"event": "crossing_mediated", "payload": written})]);
        assert_eq!((summary.records, summary.mediated), (1, 1));
    }
}
