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

/// One thing the agent read, or was refused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Crossing {
    pub session_id: String,
    pub timestamp: DateTime<Utc>,
    pub mode: CrossingMode,
    /// `claude-code`, `codex`, or whichever host observed it.
    pub host: String,
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
    /// Serialised only when true; records from before the field existed read
    /// as false and still cross the address floor and current prefix list.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub internal: bool,
    /// Present when the bytes entered the model's context and this runtime saw
    /// them. Absent for a search result, where only a title and snippet did.
    /// On a mediated fetch it covers the text delivered or withheld, which
    /// for an HTML page is the extracted text and not the markup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    /// SHA-256 over the response body as the origin served it, on a mediated
    /// fetch that received one. Equal to `content_hash` where the body was
    /// delivered as decoded; where the two differ, the transform-stage
    /// invocation recorded before the crossing carries both and ties them.
    /// Never projected onto the Content Telemetry wire: what a receiver
    /// learns is the hash of what entered context, as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieved_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_tokens: Option<u64>,
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
    }
}

/// Count one session's records: see [`SessionSummary`].
///
/// Reconstructed crossings are counted apart from the rest, in the grades and
/// in the grounding alike. A grounded total that merged them would claim this
/// runtime saw page text enter context when only a transcript says so.
pub fn summarise<'a>(records: impl IntoIterator<Item = &'a Value>) -> SessionSummary {
    let mut summary = SessionSummary::default();
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
        if record["payload"]["grounded"] == Value::Bool(true) {
            if event == Some("crossing_reconstructed") {
                summary.grounded_reconstructed += 1;
            } else {
                summary.grounded_witnessed += 1;
            }
        }
    }
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

pub struct SessionLog {
    session_id: String,
    log: EvidenceLog,
    path: PathBuf,
    /// The digest of the effective policy the process writing this log
    /// enforces, stamped on every mediated crossing it records. Set by the
    /// mediating server, which resolves its policy once at start; a hook
    /// process enforces nothing and leaves it unset.
    policy_identity: Option<String>,
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
    pub fn open(home: &Path, session_id: &str) -> std::io::Result<Self> {
        let path = home.join("sessions").join(format!("{session_id}.ndjson"));
        Ok(Self {
            session_id: session_id.to_owned(),
            log: EvidenceLog::open_append(&path)?,
            path,
            policy_identity: None,
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
        let mut record = crossing.to_record();
        // Only a mediated crossing met a policy, so only a mediated crossing
        // names the policy it met. The other two grades were captured outside
        // any enforcement path and carry no identity, exactly as they carry
        // no principal.
        if crossing.mode == CrossingMode::Mediated
            && let Some(identity) = &self.policy_identity
        {
            record["policy_identity"] = json!(identity);
        }
        self.log.append(event, record)
    }

    /// One processor invocation, recorded beside the crossing it judged. The
    /// payload is the invocation record itself
    /// (`commonmeasure_runtime::processor::INVOCATION_VERSION`); the session log adds
    /// nothing and takes nothing away.
    pub fn record_processor(&mut self, invocation: Value) -> std::io::Result<u64> {
        self.log.append("processor_invoked", invocation)
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
        self.log.append(event, record)
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
        self.log.append("nudge_issued", record)
    }

    /// The identity this edge runs under, recorded at session start from the
    /// enrolment record: the hub, the key id the hub assigned, and the
    /// key's standing (`enrolled`, or `revoked` with when and by which side).
    /// A session recorded after a revocation was learnt names the revoked
    /// key id and says so, so a reader can tell which sessions ran under a
    /// key publishers still honoured. Not written for an edge that is not
    /// enrolled: absence means no network identity, not an unknown one.
    pub fn record_edge_identity(
        &mut self,
        host: &str,
        enrolment: &crate::enrolment::EnrolmentRecord,
    ) -> std::io::Result<u64> {
        self.log.append(
            "edge_identity",
            json!({
                "session_id": self.session_id,
                "host": host,
                "timestamp": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                "hub": enrolment.hub,
                "key_id": enrolment.key_id,
                "standing": enrolment.standing(),
                "revoked_at": enrolment.revoked_at,
                "revocation": enrolment.revocation,
            }),
        )
    }

    /// Where provider credentials came from at MCP server start: the
    /// operator file's path and digest plus variable names, or the fact that
    /// no file was present. The payload is built by
    /// `commonmeasure_supply::credentials::CredentialsStatus::to_value` and by
    /// construction carries no credential value; the log adds identity and
    /// the timestamp and takes nothing away.
    pub fn record_credentials(&mut self, host: &str, payload: Value) -> std::io::Result<u64> {
        self.log.append(
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
        self.log.append(
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
        self.log.append("manifest_resolved", payload)
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
        self.log.append("prompt_sources", payload)
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
        self.log.append("context_snapshot", payload)
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

    /// An empty log is zero of everything and never a panic or an underflow.
    #[test]
    fn an_empty_session_counts_to_zero() {
        let summary = summarise(&Vec::new());
        assert_eq!(summary, SessionSummary::default());
        assert_eq!(summary.named_not_read(), 0);
    }
}
