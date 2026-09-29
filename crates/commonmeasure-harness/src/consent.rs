//! The operator's consent to report to sources whose licence demands it.
//!
//! Owner decision, 27 September 2026 (consent before an obligated crossing):
//! the consent is asked once, at install. With it, a licence's telemetry
//! reporting demand can be met in every policy scope, including one that
//! sets `allow_telemetry_egress: false`; without it, a source with a
//! reporting demand is refused and the refusal says what is missing. The
//! consent, not a scope's clearance, fills the reporting slot for such a
//! source, and a scope's clearance is never read as consent.
//!
//! The consent lives in `<home>/consent.json` and nowhere else. It is the
//! operator's own act: `commonmeasure consent agree` and `withdraw` write
//! it, and neither `deployment.json` nor a hub-managed policy can, since
//! both refuse unknown fields and neither names this file. The file is read
//! at each ruling, as `relay.json` is, so a withdrawal applies to the next
//! crossing of a server already running. A crossing admitted under consent
//! records the consent it was admitted under, and the relay reports that
//! crossing whatever the file says by the time it runs: withdrawal recalls
//! nothing, whether spooled or not yet projected.
//!
//! A file that does not read, or does not parse, is not consent. Nor is a
//! file whose writer cannot be taken to be the operator: consent counts only
//! from a regular file, opened without following a symbolic link, owned by
//! the user running Common Measure and writable by no one else. Otherwise a
//! link to a file elsewhere, or a file another local user can write, would
//! make someone else's bytes the operator's agreement. The checks read the
//! opened file's own metadata, so the file checked is the file read. The
//! error names the file and the fault, so the refusal tells the operator to
//! repair or rewrite it rather than that they never agreed. The writer
//! replaces the file atomically at mode 0600, whatever the umask.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file's name in the operator home.
pub const CONSENT_FILE: &str = "consent.json";

/// The version of [`CONSENT_TEXT`] this binary shows and records. A new
/// version is a new text: an agreement records the version agreed to, and
/// a version this binary does not know is not read as consent.
pub const CONSENT_TEXT_VERSION: &str = "1";

/// What the operator agrees to, version [`CONSENT_TEXT_VERSION`].
pub const CONSENT_TEXT: &str = "Some sources license their content only if each use of it is \
reported. If you agree, Common Measure admits those sources in every policy scope and reports \
each use of them through the telemetry receiver named in relay.json: the page address, when it \
was retrieved and entered context, its licence, its content hash and token estimate, and this \
edge's key id. Prompts, answers and page text are not sent. If you do not agree, sources that \
demand reporting are refused. Withdrawing applies to later fetches; uses already admitted are \
still reported.";

/// The command that records agreement, as a refusal and the status name it.
pub const AGREE_COMMAND: &str = "commonmeasure consent agree";

/// Where the consent lives in `home`.
pub fn path(home: &Path) -> PathBuf {
    home.join(CONSENT_FILE)
}

/// The operator's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    Agreed,
    Withdrawn,
}

/// One consent as the file holds it: the answer, when it was given and the
/// text version it was given to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Given {
    pub answer: Answer,
    pub at: DateTime<Utc>,
    pub text_version: String,
}

/// `<home>/consent.json`. Reporting is the one obligation with an install
/// consent; the file is keyed by obligation so that a later decision on
/// payment or use limits adds a member rather than a format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentFile {
    pub reporting: Given,
}

/// The reporting consent of a home as a ruling reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// No file: consent was never given in this home.
    NotGiven,
    Agreed(Given),
    Withdrawn(Given),
    /// The file exists and is not consent this binary can read or trust:
    /// it does not read or parse, or it is a link, not a regular file, owned
    /// by another user or writable by others. The text names the file and
    /// the fault.
    Unreadable(String),
}

impl Standing {
    /// Read `<home>/consent.json`.
    pub fn load(home: &Path) -> Self {
        let source = path(home);
        let bytes = match read_trusted(&source) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Self::NotGiven,
            Err(error) => return Self::Unreadable(error),
        };
        match serde_json::from_slice::<ConsentFile>(&bytes) {
            Ok(file) if file.reporting.text_version != CONSENT_TEXT_VERSION => {
                Self::Unreadable(format!(
                    "{} records consent to text version {}, which this binary does not know \
                     (it shows version {CONSENT_TEXT_VERSION})",
                    source.display(),
                    file.reporting.text_version
                ))
            }
            Ok(file) => match file.reporting.answer {
                Answer::Agreed => Self::Agreed(file.reporting),
                Answer::Withdrawn => Self::Withdrawn(file.reporting),
            },
            Err(error) => Self::Unreadable(format!(
                "{} is not a valid consent file: {error}",
                source.display()
            )),
        }
    }

    pub fn agreed(&self) -> bool {
        matches!(self, Self::Agreed(_))
    }

    /// The word the record, `status --json` and the console carry.
    pub fn state(&self) -> &'static str {
        match self {
            Self::NotGiven => "not_given",
            Self::Agreed(_) => "agreed",
            Self::Withdrawn(_) => "withdrawn",
            Self::Unreadable(_) => "unreadable",
        }
    }

    /// What a reporting ruling records of this standing.
    pub fn on_record(&self) -> ConsentOnRecord {
        let given = match self {
            Self::Agreed(given) | Self::Withdrawn(given) => Some(given),
            Self::NotGiven | Self::Unreadable(_) => None,
        };
        ConsentOnRecord {
            state: self.state().to_owned(),
            at: given.map(|given| given.at),
            text_version: given.map(|given| given.text_version.clone()),
        }
    }

    /// Why a reporting demand is unmet for want of consent, or `None` where
    /// the operator agreed. `file` is the consent file as the caller may be
    /// told it. The text names the three things the operator is missing:
    /// the source needs reporting (the caller's sentence says which source),
    /// it has not been agreed to, and how to agree.
    pub fn unmet_reason(&self, file: &str) -> Option<String> {
        match self {
            Self::Agreed(_) => None,
            Self::NotGiven => Some(format!(
                "the operator has not agreed to report to sources that require it; agree with \
                 `{AGREE_COMMAND}`"
            )),
            Self::Withdrawn(given) => Some(format!(
                "the operator withdrew consent to report to sources that require it on {}; \
                 agree again with `{AGREE_COMMAND}`",
                given.at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
            )),
            Self::Unreadable(error) => Some(format!(
                "the reporting consent in {file} cannot be used ({error}), and such a file is \
                 not consent; repair the file or record consent again with `{AGREE_COMMAND}`"
            )),
        }
    }
}

/// The consent a reporting ruling was made under, on the crossing's record.
/// The relay reports a crossing admitted under `agreed` consent whatever
/// the scope clears, and nothing else on the strength of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentOnRecord {
    /// `agreed`, `withdrawn`, `not_given` or `unreadable`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_version: Option<String>,
}

/// The bytes of the consent file at `source`, `None` where there is none,
/// or why the file is not consent.
///
/// The file is opened without following a symbolic link (`O_NOFOLLOW`), and
/// its type, owner and mode are read from the open descriptor, so no rename
/// between the check and the read can put other bytes behind a checked name.
/// `O_NONBLOCK` keeps a FIFO planted at the name from stalling a ruling; it
/// is then refused as not a regular file.
#[cfg(unix)]
fn read_trusted(source: &Path) -> Result<Option<Vec<u8>>, String> {
    use std::io::Read as _;
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
    let named = source.display();
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(source)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        // Refused links are named as links: the errno for one differs
        // between platforms (ELOOP, EMLINK), and the name is what the
        // operator can act on.
        Err(_) if source.is_symlink() => {
            return Err(format!(
                "{named} is a symbolic link; consent counts only from a regular file in the \
                 Edge home"
            ));
        }
        Err(error) => return Err(format!("cannot read {named}: {error}")),
    };
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot read the metadata of {named}: {error}"))?;
    if !metadata.file_type().is_file() {
        return Err(format!("{named} is not a regular file"));
    }
    // SAFETY: geteuid has no preconditions and cannot fail.
    let user = unsafe { libc::geteuid() };
    if metadata.uid() != user {
        return Err(format!(
            "{named} is owned by uid {}, not by the user running Common Measure (uid {user})",
            metadata.uid()
        ));
    }
    let mode = metadata.mode() & 0o777;
    let writable_by = match (mode & 0o020 != 0, mode & 0o002 != 0) {
        (_, true) => Some("others"),
        (true, false) => Some("its group"),
        (false, false) => None,
    };
    if let Some(writers) = writable_by {
        return Err(format!(
            "{named} is writable by {writers} (mode {mode:03o}); consent counts only from a \
             file no one else can write"
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {named}: {error}"))?;
    Ok(Some(bytes))
}

/// Where the platform has no unix owner or mode bits, a symbolic link is
/// still refused, checked on the name before the read.
#[cfg(not(unix))]
fn read_trusted(source: &Path) -> Result<Option<Vec<u8>>, String> {
    let named = source.display();
    match std::fs::symlink_metadata(source) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read {named}: {error}")),
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(format!(
                "{named} is a symbolic link; consent counts only from a regular file in the \
                 Edge home"
            ));
        }
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(format!("{named} is not a regular file"));
        }
        Ok(_) => {}
    }
    match std::fs::read(source) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("cannot read {named}: {error}")),
    }
}

/// Record `answer` at `at` against the current text. The file is replaced
/// atomically, so a ruling reads the old answer or the new one, never part
/// of either, and at mode 0600 whatever the umask, so that
/// [`Standing::load`] trusts it. A link or an unsafe file at the name is
/// replaced, not written through.
pub fn record(home: &Path, answer: Answer, at: DateTime<Utc>) -> Result<Given, String> {
    let given = Given {
        answer,
        at,
        text_version: CONSENT_TEXT_VERSION.to_owned(),
    };
    let encoded = serde_json::to_vec_pretty(&ConsentFile {
        reporting: given.clone(),
    })
    .map_err(|error| format!("serialise the consent: {error}"))?;
    std::fs::create_dir_all(home).map_err(|error| format!("create {}: {error}", home.display()))?;
    crate::declaration::replace_with_mode(&path(home), &encoded, 0o600)?;
    Ok(given)
}

/// A source this home refused for want of reporting consent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefusedSource {
    /// The host the refused crossing named.
    pub source: String,
    pub refusals: u64,
    pub last_refused_at: Option<String>,
}

/// The sources this home's session logs refused because they need reporting
/// and the operator had not agreed (a crossing's reporting ruling marked
/// `consent_needed`), most refused first, and how many logs did not read.
/// A log that does not read is counted, not skipped silently: the list may
/// then be short, and the caller says so.
pub fn refused_sources(home: &Path) -> Result<(Vec<RefusedSource>, usize), String> {
    let mut by_source: std::collections::BTreeMap<String, RefusedSource> = Default::default();
    let mut unreadable = 0;
    for log in crate::SessionLog::list(home)
        .map_err(|error| format!("list {}: {error}", home.join("sessions").display()))?
    {
        let Ok(records) = crate::SessionLog::read(&log) else {
            unreadable += 1;
            continue;
        };
        for record in records.iter().filter(|record| {
            record["event"] == "crossing_refused"
                && record["payload"]["declarations"]["reporting"]["consent_needed"] == true
        }) {
            let payload = &record["payload"];
            let source = payload["host_name"]
                .as_str()
                .filter(|host| !host.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    crate::grounding::host_of(payload["url"].as_str().unwrap_or(""))
                });
            let entry = by_source
                .entry(source.clone())
                .or_insert_with(|| RefusedSource {
                    source,
                    refusals: 0,
                    last_refused_at: None,
                });
            entry.refusals += 1;
            let at = payload["timestamp"].as_str().map(str::to_owned);
            if at > entry.last_refused_at {
                entry.last_refused_at = at;
            }
        }
    }
    let mut sources: Vec<RefusedSource> = by_source.into_values().collect();
    sources.sort_by(|a, b| b.refusals.cmp(&a.refusals).then(a.source.cmp(&b.source)));
    Ok((sources, unreadable))
}

/// The reporting consent block `consent show --json`, `status --json`,
/// `doctor --json` and the console carry: the standing, the file, the
/// command to agree, and, while consent is not agreed, the sources refused
/// for want of it. Once agreed the list is empty: those sources are
/// admitted from the next fetch.
pub fn report(home: &Path) -> serde_json::Value {
    let standing = Standing::load(home);
    let on_record = standing.on_record();
    let mut report = serde_json::json!({
        "state": on_record.state,
        "at": on_record.at,
        "text_version": on_record.text_version,
        "error": match &standing {
            Standing::Unreadable(error) => Some(error),
            _ => None,
        },
        "file": path(home),
        "agree_command": AGREE_COMMAND,
        "current_text_version": CONSENT_TEXT_VERSION,
    });
    if standing.agreed() {
        report["refused_sources"] = serde_json::json!([]);
        report["refused_source_count"] = serde_json::json!(0);
        report["refusals"] = serde_json::json!(0);
        return report;
    }
    match refused_sources(home) {
        Ok((sources, unreadable)) => {
            report["refusals"] = serde_json::json!(sources.iter().map(|s| s.refusals).sum::<u64>());
            report["refused_source_count"] = serde_json::json!(sources.len());
            report["refused_sources"] = serde_json::json!(sources);
            if unreadable > 0 {
                report["unreadable_sessions"] = serde_json::json!(unreadable);
            }
        }
        Err(error) => report["refused_sources_error"] = serde_json::json!(error),
    }
    report
}

/// The findings a [`report`] gives, in the order `status` and `doctor`
/// print them.
pub fn findings(report: &serde_json::Value) -> Vec<commonmeasure_types::Finding> {
    use commonmeasure_types::Finding;
    let mut findings = Vec::new();
    let refusals_word = |n: u64| if n == 1 { "refusal" } else { "refusals" };
    let sources = report["refused_sources"]
        .as_array()
        .filter(|sources| !sources.is_empty());
    // The command is named once, on the line the operator acts on.
    let agree = if sources.is_some() {
        String::new()
    } else {
        format!(". Agree with: {AGREE_COMMAND}")
    };
    match report["state"].as_str() {
        Some("agreed") => {
            findings.push(Finding::ok(format!(
                "reporting consent agreed at {} (consent text {})",
                report["at"].as_str().unwrap_or("an unrecorded time"),
                report["text_version"].as_str().unwrap_or("unrecorded")
            )));
            return findings;
        }
        Some("unreadable") => findings.push(Finding::attention(format!(
            "reporting consent unreadable ({}), which is not consent: sources whose licence \
             demands reporting are refused. Record it again with: {AGREE_COMMAND}",
            report["error"].as_str().unwrap_or("no error recorded")
        ))),
        Some("withdrawn") => findings.push(Finding::note(format!(
            "reporting consent withdrawn at {}: sources whose licence demands reporting are \
             refused{agree}",
            report["at"].as_str().unwrap_or("an unrecorded time")
        ))),
        _ => findings.push(Finding::note(format!(
            "reporting consent not given: sources whose licence demands reporting are \
             refused{agree}"
        ))),
    }
    if let Some(sources) = sources {
        const SHOWN: usize = 5;
        let mut named: Vec<String> = sources
            .iter()
            .take(SHOWN)
            .map(|source| {
                let n = source["refusals"].as_u64().unwrap_or(0);
                format!(
                    "{} ({n} {})",
                    source["source"].as_str().unwrap_or("unnamed"),
                    refusals_word(n)
                )
            })
            .collect();
        if sources.len() > SHOWN {
            named.push(format!("{} more", sources.len() - SHOWN));
        }
        findings.push(Finding::attention(format!(
            "{} {} that {} reporting refused for want of reporting consent: {}. Agree with: \
             {AGREE_COMMAND}",
            sources.len(),
            if sources.len() == 1 {
                "source"
            } else {
                "sources"
            },
            if sources.len() == 1 { "needs" } else { "need" },
            named.join(", ")
        )));
    }
    if let Some(n) = report["unreadable_sessions"].as_u64() {
        findings.push(Finding::unknown(format!(
            "{n} session {} did not read, so the sources refused for want of reporting consent \
             may be more than listed",
            if n == 1 { "log" } else { "logs" }
        )));
    }
    if let Some(error) = report["refused_sources_error"].as_str() {
        findings.push(Finding::unknown(format!(
            "sources refused for want of reporting consent unknown: {error}"
        )));
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_home_without_the_file_has_not_given_consent() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(Standing::load(home.path()), Standing::NotGiven);
        assert!(
            Standing::NotGiven
                .unmet_reason("consent.json")
                .unwrap()
                .contains(AGREE_COMMAND)
        );
    }

    #[test]
    fn agreement_and_withdrawal_round_trip_with_their_time_and_text_version() {
        let home = tempfile::tempdir().unwrap();
        let at = "2026-09-29T10:00:00Z".parse().unwrap();
        record(home.path(), Answer::Agreed, at).unwrap();
        let standing = Standing::load(home.path());
        assert!(standing.agreed());
        assert_eq!(standing.unmet_reason("consent.json"), None);
        let on_record = standing.on_record();
        assert_eq!(on_record.state, "agreed");
        assert_eq!(on_record.at, Some(at));
        assert_eq!(
            on_record.text_version.as_deref(),
            Some(CONSENT_TEXT_VERSION)
        );

        let later = "2026-09-30T10:00:00Z".parse().unwrap();
        record(home.path(), Answer::Withdrawn, later).unwrap();
        let standing = Standing::load(home.path());
        assert!(!standing.agreed());
        assert_eq!(standing.state(), "withdrawn");
        assert!(
            standing
                .unmet_reason("consent.json")
                .unwrap()
                .contains("withdrew consent")
        );
    }

    const AGREED: &[u8] =
        br#"{"reporting":{"answer":"agreed","at":"2026-09-29T10:00:00Z","text_version":"1"}}"#;

    fn unreadable(home: &Path) -> String {
        match Standing::load(home) {
            Standing::Unreadable(error) => error,
            other => panic!("{other:?}"),
        }
    }

    /// Review F1 (P1), 29 September 2026: consent counts only from a
    /// regular file of this user that no one else can write. A symbolic
    /// link to a valid agreement, a link to a world-writable one, and a
    /// regular file writable by others or by its group are each refused
    /// with the fault named, never read as "not agreed"; a FIFO at the name
    /// is refused without blocking. The target of a link is left as it was
    /// when the operator then agrees, and the new file is 0600.
    #[cfg(unix)]
    #[test]
    fn a_link_or_a_file_others_can_write_is_not_consent_and_says_why() {
        use std::os::unix::fs::PermissionsExt as _;
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let file = path(home.path());

        let valid = elsewhere.path().join("valid.json");
        std::fs::write(&valid, AGREED).unwrap();
        std::fs::set_permissions(&valid, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&valid, &file).unwrap();
        let error = unreadable(home.path());
        assert!(error.contains("is a symbolic link"), "{error}");
        let reason = Standing::load(home.path())
            .unmet_reason("consent.json")
            .unwrap();
        assert!(
            reason.contains("is a symbolic link") && !reason.contains("has not agreed"),
            "{reason}"
        );

        let writable = elsewhere.path().join("writable.json");
        std::fs::write(&writable, AGREED).unwrap();
        std::fs::set_permissions(&writable, std::fs::Permissions::from_mode(0o666)).unwrap();
        std::fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink(&writable, &file).unwrap();
        assert!(unreadable(home.path()).contains("is a symbolic link"));

        std::fs::remove_file(&file).unwrap();
        std::fs::write(&file, AGREED).unwrap();
        for (mode, writers) in [(0o666, "others"), (0o602, "others"), (0o620, "its group")] {
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
            let error = unreadable(home.path());
            assert!(
                error.contains(&format!("is writable by {writers} (mode {mode:03o})")),
                "{error}"
            );
        }
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(Standing::load(home.path()).agreed());

        std::fs::remove_file(&file).unwrap();
        let fifo = std::ffi::CString::new(file.to_str().unwrap()).unwrap();
        // SAFETY: a valid NUL-terminated path; mkfifo has no other precondition.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(unreadable(home.path()).contains("is not a regular file"));
        std::fs::remove_file(&file).unwrap();

        std::os::unix::fs::symlink(&writable, &file).unwrap();
        record(home.path(), Answer::Agreed, chrono::Utc::now()).unwrap();
        let metadata = std::fs::symlink_metadata(&file).unwrap();
        assert!(metadata.file_type().is_file());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read(&writable).unwrap(), AGREED);
        assert!(Standing::load(home.path()).agreed());
    }

    /// Review F1: a regular file at the name owned by another user is not
    /// consent. A file of another user is placed by hard link, which a
    /// non-root user can make only where the platform allows linking a file
    /// it does not own on the same volume (macOS does; Linux with
    /// `protected_hardlinks` does not); where it cannot, the test says so
    /// and checks nothing.
    #[cfg(unix)]
    #[test]
    fn a_file_owned_by_another_user_is_not_consent() {
        use std::os::unix::fs::MetadataExt as _;
        // SAFETY: geteuid has no preconditions.
        let user = unsafe { libc::geteuid() };
        let home = tempfile::tempdir().unwrap();
        let file = path(home.path());
        // World-readable, so the open succeeds and the refusal is the owner
        // check on the opened file.
        let foreign = ["/private/etc/hosts", "/etc/hosts", "/etc/passwd"]
            .into_iter()
            .map(Path::new)
            .filter(|candidate| {
                std::fs::symlink_metadata(candidate).is_ok_and(|metadata| {
                    metadata.is_file() && metadata.uid() != user && metadata.mode() & 0o004 != 0
                })
            })
            .find(|candidate| std::fs::hard_link(candidate, &file).is_ok());
        let Some(foreign) = foreign else {
            eprintln!("no file of another user can be linked here; ownership check not exercised");
            return;
        };
        let error = unreadable(home.path());
        let owner = std::fs::metadata(foreign).unwrap().uid();
        assert!(
            error.contains(&format!(
                "is owned by uid {owner}, not by the user running Common Measure (uid {user})"
            )),
            "{error}"
        );
    }

    /// Review F1: the writer sets mode 0600 whatever the umask, so a file
    /// `consent agree` or the installer writes under umask 000 is trusted.
    /// The body runs in a child of this test binary whose umask is set to
    /// 000 between fork and exec, so this process's umask is untouched.
    #[cfg(unix)]
    #[test]
    fn the_consent_file_is_written_at_0600_under_umask_000() {
        use std::os::unix::fs::PermissionsExt as _;
        use std::os::unix::process::CommandExt as _;
        const NAME: &str = "consent::tests::the_consent_file_is_written_at_0600_under_umask_000";
        const CHILD: &str = "COMMONMEASURE_CONSENT_UMASK_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .args(["--exact", NAME, "--test-threads=1"])
                .env(CHILD, "1");
            // SAFETY: `umask` is async-signal-safe and changes only the
            // child's own process state, between fork and exec.
            unsafe {
                child.pre_exec(|| {
                    libc::umask(0o000);
                    Ok(())
                });
            }
            let output = child.output().unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(output.status.success(), "{stdout}");
            assert!(
                stdout.contains("1 passed"),
                "the child ran the test: {stdout}"
            );
            return;
        }
        let home = tempfile::tempdir().unwrap();
        record(home.path(), Answer::Agreed, chrono::Utc::now()).unwrap();
        let mode = std::fs::metadata(path(home.path()))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert!(Standing::load(home.path()).agreed());
    }

    /// Consent before an obligated crossing, lane call 4: an unreadable or
    /// malformed file is not consent, and the reason names the error rather
    /// than saying consent was never given.
    #[test]
    fn a_malformed_or_unknown_version_file_is_unreadable_and_named() {
        let home = tempfile::tempdir().unwrap();
        for (bytes, fault) in [
            (&b"{not json"[..], "is not a valid consent file"),
            (
                br#"{"reporting":{"answer":"agreed","at":"2026-09-29T10:00:00Z","text_version":"1"},"payment":{}}"#,
                "unknown field",
            ),
            (
                br#"{"reporting":{"answer":"agreed","at":"2026-09-29T10:00:00Z","text_version":"9"}}"#,
                "text version 9",
            ),
            (br#"{"reporting":{"answer":"maybe","at":"2026-09-29T10:00:00Z","text_version":"1"}}"#, "unknown variant"),
        ] {
            std::fs::write(path(home.path()), bytes).unwrap();
            let standing = Standing::load(home.path());
            let Standing::Unreadable(error) = &standing else {
                panic!("{standing:?}");
            };
            assert!(error.contains(fault), "{error}");
            let reason = standing.unmet_reason("consent.json").unwrap();
            assert!(reason.contains(fault) && !reason.contains("has not agreed"), "{reason}");
        }
    }
}
