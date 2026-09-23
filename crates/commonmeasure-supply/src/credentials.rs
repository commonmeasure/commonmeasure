//! The operator credentials file: `$COMMONMEASURE_HOME/credentials.env`.
//!
//! The mediator runs across every working directory the operator has, so its
//! credentials are operator state and live beside `policy.json` — never in a
//! checkout. Nothing here reads a file relative to the working directory: a
//! cloned repository must not be able to plant a `.env` and have the mediator
//! silently use attacker-chosen keys or endpoints. The repository-root `.env`
//! remains a development convenience for scripts that source it explicitly.
//!
//! Precedence is fixed and not configurable: the process environment always
//! wins. The file fills only variables the launching shell left absent or
//! empty, so a file can add a credential but can never override what the
//! operator's shell chose. The file is parsed literally as `KEY=VALUE` lines
//! — it is never sourced through a shell, so it can execute nothing.
//!
//! A present file that cannot be used — unreadable, unparseable, readable by
//! other users, or holding an unrendered secret reference — fails loudly with
//! a named reason, exactly as an invalid `policy.json` does. An absent file
//! is not an error: it is the ordinary state of a machine that keeps its
//! credentials in the launching shell.

use commonmeasure_types::canonical::sha256_digest;
use std::path::{Path, PathBuf};

/// The file name under the operator home. One name on every platform, so a
/// dossier and an error message can spell it without qualification.
pub const CREDENTIALS_FILE: &str = "credentials.env";

/// A parsed credentials file. Values are deliberately private: they leave
/// this module only into the process environment, never into a record, a
/// display impl or an error.
pub struct CredentialsFile {
    path: PathBuf,
    sha256: String,
    entries: Vec<(String, String)>,
}

/// Which variables the file supplied and which the environment already held,
/// by name only. This is what evidence records: where each variable came
/// from, never what it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialsPlan {
    /// Named in the file and absent (or empty) in the environment: the file
    /// supplies these.
    pub applied: Vec<String>,
    /// Named in the file but already set in the environment, which wins.
    pub shadowed: Vec<String>,
}

/// The provenance of provider credentials for one process, recordable as
/// evidence. Always names the path that was consulted, whether or not a file
/// was there.
#[derive(Debug, Clone)]
pub struct CredentialsStatus {
    /// Where the operator credentials file lives (or would live).
    pub path: PathBuf,
    /// `None` when no file exists at `path`.
    pub loaded: Option<LoadedCredentials>,
}

/// What a present file contributed: its digest and the variable names it
/// supplied or that the environment shadowed. Never a value.
#[derive(Debug, Clone)]
pub struct LoadedCredentials {
    pub sha256: String,
    pub applied: Vec<String>,
    pub shadowed: Vec<String>,
}

impl CredentialsStatus {
    /// The recordable shape: path and digest, names only, never a value.
    pub fn to_value(&self) -> serde_json::Value {
        match &self.loaded {
            Some(loaded) => serde_json::json!({
                "path": self.path.display().to_string(),
                "present": true,
                "sha256": loaded.sha256,
                "applied": loaded.applied,
                "shadowed": loaded.shadowed,
            }),
            None => serde_json::json!({
                "path": self.path.display().to_string(),
                "present": false,
            }),
        }
    }
}

impl CredentialsFile {
    /// Read and parse `home/credentials.env`. `Ok(None)` when the file does
    /// not exist; every other obstacle is a named error, because a file the
    /// operator wrote and the mediator quietly ignored would be exactly the
    /// silent failure this path exists to refuse.
    pub fn load(home: &Path) -> Result<Option<Self>, String> {
        let path = home.join(CREDENTIALS_FILE);
        if !path.exists() {
            return Ok(None);
        }
        check_permissions(&path)?;
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("{} cannot be read: {error}", path.display()))?;
        let text = String::from_utf8(bytes.clone())
            .map_err(|_| format!("{} is not UTF-8", path.display()))?;
        let entries = parse(&text, &path)?;
        Ok(Some(Self {
            path,
            sha256: sha256_digest(&bytes),
            entries,
        }))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Split this file's variables by whether the environment already
    /// supplies them. Pure: `already_set` is asked, the environment is not
    /// touched, so precedence is testable without mutating process state.
    pub fn plan(&self, already_set: impl Fn(&str) -> bool) -> CredentialsPlan {
        let (shadowed, applied): (Vec<_>, Vec<_>) =
            self.entries.iter().partition(|(name, _)| already_set(name));
        CredentialsPlan {
            applied: applied.into_iter().map(|(name, _)| name.clone()).collect(),
            shadowed: shadowed.into_iter().map(|(name, _)| name.clone()).collect(),
        }
    }

    /// The values for the named variables. Crate-visible on purpose: only
    /// `apply` moves values, and it moves them into the process environment
    /// and nowhere else.
    fn value_of(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(entry, _)| entry == name)
            .map(|(_, value)| value.as_str())
    }
}

impl std::fmt::Debug for CredentialsFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialsFile")
            .field("path", &self.path)
            .field("sha256", &self.sha256)
            .field(
                "entries",
                &self
                    .entries
                    .iter()
                    .map(|(name, _)| format!("{name}=«redacted»"))
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Load the operator credentials file and apply it to this process's
/// environment, under the fixed precedence: a variable the environment
/// already holds (non-empty) is never touched.
///
/// Must be called at process start, before any other thread exists — setting
/// environment variables is unsound alongside concurrent reads, which is why
/// the standard library marks it unsafe.
pub fn apply(home: &Path) -> Result<CredentialsStatus, String> {
    let path = home.join(CREDENTIALS_FILE);
    let Some(file) = CredentialsFile::load(home)? else {
        return Ok(CredentialsStatus { path, loaded: None });
    };
    let plan = file.plan(environment_supplies);
    for name in &plan.applied {
        let value = file
            .value_of(name)
            .expect("an applied name came from this file's own entries");
        // SAFETY: called at process start before any thread is spawned, per
        // this function's contract; no concurrent environment access exists.
        unsafe { std::env::set_var(name, value) };
    }
    Ok(CredentialsStatus {
        path,
        loaded: Some(LoadedCredentials {
            sha256: file.sha256.clone(),
            applied: plan.applied,
            shadowed: plan.shadowed,
        }),
    })
}

/// Whether the environment already supplies `name`. Empty and whitespace-only
/// values do not count, matching `supplier_from_environment`'s own reading:
/// a variable exported as empty is a placeholder, not a credential, and the
/// file may fill it.
pub(crate) fn environment_supplies(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| !value.trim().is_empty())
}

/// One credential a hub released to this edge
/// (`docs/contracts/supplier-credentials.md` §Release). The value is private
/// to this module: it leaves only into an adapter's request header, through
/// [`crate::supplier_with_released`].
pub struct Released {
    pub connection_id: String,
    pub provider: String,
    pub variable: String,
    /// When the hub last replaced the value, as the hub stated it.
    pub rotated_at: Option<String>,
    /// When the fetch that last served this credential completed.
    pub fetched_at: String,
    value: String,
}

impl Released {
    /// `None` unless `provider` is a remote provider, `variable` is the one
    /// [`crate::required_variable`] names for it and `value` is not blank.
    /// The corpus root and the skill catalogue are paths on this machine that
    /// decide what is read and what is executed, so a hub never supplies them.
    pub fn new(
        connection_id: &str,
        provider: &str,
        variable: &str,
        value: &str,
        rotated_at: Option<String>,
        fetched_at: &str,
    ) -> Option<Self> {
        let remote = crate::remote_adapter(provider, None, "").is_some();
        (remote
            && crate::required_variable(provider) == Some(variable)
            && !value.trim().is_empty()
            && !connection_id.is_empty())
        .then(|| Self {
            connection_id: connection_id.to_owned(),
            provider: provider.to_owned(),
            variable: variable.to_owned(),
            rotated_at,
            fetched_at: fetched_at.to_owned(),
            value: value.to_owned(),
        })
    }

    /// Whether `other` is the same credential with the same value: what makes
    /// a fetch `unchanged`.
    pub fn same_as(&self, other: &Self) -> bool {
        self.connection_id == other.connection_id
            && self.provider == other.provider
            && self.variable == other.variable
            && self.rotated_at == other.rotated_at
            && self.value == other.value
    }

    fn name(&self) -> ReleasedName {
        ReleasedName {
            connection_id: self.connection_id.clone(),
            provider: self.provider.clone(),
            variable: self.variable.clone(),
            fetched_at: self.fetched_at.clone(),
            rotated_at: self.rotated_at.clone(),
        }
    }
}

impl std::fmt::Debug for Released {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Released")
            .field("connection_id", &self.connection_id)
            .field("provider", &self.provider)
            .field("variable", &self.variable)
            .field("rotated_at", &self.rotated_at)
            .field("fetched_at", &self.fetched_at)
            .field("value", &"«redacted»")
            .finish()
    }
}

/// What evidence says of a released credential: which connection supplied
/// which variable and when it was fetched. Never the value.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReleasedName {
    pub connection_id: String,
    pub provider: String,
    pub variable: String,
    pub fetched_at: String,
    /// When the hub last replaced the value. Not part of the evidence shape,
    /// which the contract fixes at the four members above.
    #[serde(skip)]
    pub rotated_at: Option<String>,
}

/// Hub-released credentials split by whether this process uses them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReleasedStanding {
    /// No local source supplies the variable, so the released value is used.
    pub in_use: Vec<ReleasedName>,
    /// The environment, or `credentials.env` applied into it at start,
    /// already holds the variable and wins.
    pub shadowed: Vec<ReleasedName>,
    /// Held past the store's maximum age and no longer used, whatever the
    /// environment holds: no fetch has confirmed these for that long.
    pub expired: Vec<ReleasedName>,
    /// The store's maximum age, which `expired` entries have passed.
    pub max_age: std::time::Duration,
}

/// What a store reads the time from. Monotonic, so a wall-clock correction
/// neither extends nor ends a release. Injected in tests.
pub type Clock = std::sync::Arc<dyn Fn() -> std::time::Instant + Send + Sync>;

/// The in-process store of hub-released credentials, read at adapter
/// construction after the process environment. `credentials.env` is applied
/// into the environment at start, so the fixed precedence is environment,
/// then file, then hub release, and a hub can never override a variable the
/// operator supplied locally.
///
/// A handle: clones share one set. The hosted service's custody thread
/// replaces the set while sessions read it, which is why released values are
/// not written to the process environment (see [`apply`]). Nothing here
/// touches a file.
///
/// A set is used for at most `max_age` after the last [`Self::replace`],
/// which is the last fetch the hub answered with a release. The age is
/// checked where a value is read and not by whatever fetches, so a fetch
/// thread that has died, a fetch that refuses to try and an unreachable hub
/// all end the same way: the variable is missing and the provider is
/// unavailable (`docs/FAIL-POLICY.md` §5). An expired set is still named, by
/// [`Self::expired`], so the record can say why.
#[derive(Clone)]
pub struct ReleasedStore {
    held: std::sync::Arc<std::sync::RwLock<HeldSet>>,
    max_age: std::time::Duration,
    clock: Clock,
}

#[derive(Default)]
struct HeldSet {
    released: Vec<Released>,
    /// When the hub last confirmed the set. `None` until the first release.
    confirmed: Option<std::time::Instant>,
}

impl ReleasedStore {
    /// An empty store whose sets are used for at most `max_age` after the
    /// fetch that last confirmed them.
    pub fn new(max_age: std::time::Duration) -> Self {
        Self::with_clock(max_age, std::sync::Arc::new(std::time::Instant::now))
    }

    /// [`Self::new`] reading the time from `clock`, so expiry is testable
    /// without waiting for it.
    pub fn with_clock(max_age: std::time::Duration, clock: Clock) -> Self {
        Self {
            held: std::sync::Arc::default(),
            max_age,
            clock,
        }
    }

    /// The longest a set is used after the fetch that last confirmed it.
    pub fn max_age(&self) -> std::time::Duration {
        self.max_age
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HeldSet> {
        self.held
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether `set` holds credentials no fetch has confirmed for `max_age`.
    fn is_expired(&self, set: &HeldSet) -> bool {
        !set.released.is_empty()
            && set
                .confirmed
                .is_none_or(|confirmed| (self.clock)().duration_since(confirmed) >= self.max_age)
    }

    /// Replace the held set with `released`, confirmed now. Returns whether
    /// anything differed from the set in use, so a release that follows an
    /// expiry counts as a change; fetch times are taken from `released`
    /// either way.
    pub fn replace(&self, released: Vec<Released>) -> bool {
        let mut held = self
            .held
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let in_use: &[Released] = if self.is_expired(&held) {
            &[]
        } else {
            &held.released
        };
        let changed = in_use.len() != released.len()
            || in_use
                .iter()
                .zip(&released)
                .any(|(before, after)| !before.same_as(after));
        *held = HeldSet {
            released,
            confirmed: Some((self.clock)()),
        };
        changed
    }

    /// Drop every held credential. Returns whether any was in use.
    pub fn clear(&self) -> bool {
        self.replace(Vec::new())
    }

    /// The credentials in use by name, in the order the hub served them.
    /// Empty once the set has expired.
    pub fn names(&self) -> Vec<ReleasedName> {
        let held = self.read();
        if self.is_expired(&held) {
            return Vec::new();
        }
        held.released.iter().map(Released::name).collect()
    }

    /// The credentials still in memory that are past `max_age` and no longer
    /// used. Empty while the set is in use.
    pub fn expired(&self) -> Vec<ReleasedName> {
        let held = self.read();
        if !self.is_expired(&held) {
            return Vec::new();
        }
        held.released.iter().map(Released::name).collect()
    }

    /// The held credentials split by whether the environment shadows them.
    pub fn standing(&self) -> ReleasedStanding {
        self.standing_under(environment_supplies)
    }

    /// [`Self::standing`] with the environment asked through `already_set`,
    /// so precedence is testable without mutating process state.
    pub fn standing_under(&self, already_set: impl Fn(&str) -> bool) -> ReleasedStanding {
        let (shadowed, in_use) = self
            .names()
            .into_iter()
            .partition(|name| already_set(&name.variable));
        ReleasedStanding {
            in_use,
            shadowed,
            expired: self.expired(),
            max_age: self.max_age,
        }
    }

    /// The released value for `variable`, or `None` once the set has
    /// expired. Crate-visible on purpose: only adapter construction reads a
    /// value.
    pub(crate) fn value_of(&self, variable: &str) -> Option<String> {
        let held = self.read();
        if self.is_expired(&held) {
            return None;
        }
        held.released
            .iter()
            .find(|released| released.variable == variable)
            .map(|released| released.value.clone())
    }
}

impl std::fmt::Debug for ReleasedStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReleasedStore")
            .field("held", &self.names())
            .field("expired", &self.expired())
            .field("max_age", &self.max_age)
            .finish()
    }
}

/// Refuse a credentials file other users on the machine can read. Unix only:
/// the bundled Windows binary has no mode bits to check, and this check is a
/// floor, not the security model.
#[cfg(unix)]
fn check_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("{} cannot be inspected: {error}", path.display()))?;
    if metadata.mode() & 0o077 != 0 {
        return Err(format!(
            "{} is readable by other users (mode {:o}); credentials must be private. \
             Fix it with: chmod 600 {}",
            path.display(),
            metadata.mode() & 0o777,
            path.display()
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Parse `KEY=VALUE` lines. Blank lines and `#` comments are skipped, a
/// leading `export ` is tolerated, and one matching pair of single or double
/// quotes around a value is stripped. Anything else is a named error for the
/// whole file: a line the operator wrote and the parser guessed at would be a
/// credential silently misread.
fn parse(text: &str, path: &Path) -> Result<Vec<(String, String)>, String> {
    let mut entries: Vec<(String, String)> = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((name, value)) = line.split_once('=') else {
            return Err(format!(
                "{} line {} is not KEY=VALUE: {raw:?}",
                path.display(),
                index + 1
            ));
        };
        let name = name.trim();
        if name.is_empty()
            || !name
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            return Err(format!(
                "{} line {} does not name a variable: {raw:?}",
                path.display(),
                index + 1
            ));
        }
        let value = unquote(value.trim());
        if value.starts_with("gopass:") {
            // `.env.template` spells values as secret-store references for a
            // renderer to resolve. A file holding one was copied without
            // rendering, and sending the reference itself as an API key would
            // fail every call while looking configured.
            return Err(format!(
                "{} sets {name} to an unrendered secret reference ({}). Render the template \
                 (for example with gopass) so the file holds the credential itself.",
                path.display(),
                "gopass:…"
            ));
        }
        if value.is_empty() {
            // An empty value is a placeholder (`.env.example` ships them), so
            // the variable stays unconfigured rather than set-but-useless.
            continue;
        }
        if let Some(existing) = entries.iter_mut().find(|(entry, _)| entry == name) {
            // Last assignment wins, matching what sourcing the file would do.
            existing.1 = value;
        } else {
            entries.push((name.to_owned(), value));
        }
    }
    Ok(entries)
}

/// Strip one matching pair of quotes. No escape processing: a credential is
/// an opaque token, and inventing escape semantics would corrupt any key that
/// happens to contain a backslash.
fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'"' || bytes[0] == b'\'')
        && bytes[bytes.len() - 1] == bytes[0]
    {
        value[1..value.len() - 1].to_owned()
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_private(home: &Path, content: &str) -> PathBuf {
        let path = home.join(CREDENTIALS_FILE);
        std::fs::write(&path, content).expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        }
        path
    }

    #[test]
    fn an_absent_file_is_the_ordinary_state_not_an_error() {
        let home = tempfile::tempdir().expect("tempdir");
        assert!(
            CredentialsFile::load(home.path())
                .expect("absent is fine")
                .is_none()
        );
        let status = apply(home.path()).expect("absent is fine");
        assert!(status.loaded.is_none());
        assert_eq!(status.path, home.path().join(CREDENTIALS_FILE));
        assert_eq!(status.to_value()["present"], false);
    }

    #[test]
    fn parses_comments_export_prefixes_and_quotes() {
        let home = tempfile::tempdir().expect("tempdir");
        write_private(
            home.path(),
            "# operator credentials\n\
             EXA_API_KEY=plain\n\
             export TAVILY_API_KEY=\"quoted\"\n\
             YOU_API_KEY='single'\n\
             \n\
             EMPTY_PLACEHOLDER=\n",
        );
        let file = CredentialsFile::load(home.path())
            .expect("parses")
            .expect("present");
        assert_eq!(file.value_of("EXA_API_KEY"), Some("plain"));
        assert_eq!(file.value_of("TAVILY_API_KEY"), Some("quoted"));
        assert_eq!(file.value_of("YOU_API_KEY"), Some("single"));
        assert_eq!(
            file.value_of("EMPTY_PLACEHOLDER"),
            None,
            "an empty value is a placeholder, not a credential"
        );
        assert!(file.sha256().starts_with("sha256:"));
    }

    /// The environment always wins; the file fills only what is absent. Pure
    /// via `plan`, so the test never touches the process environment.
    #[test]
    fn the_environment_wins_and_the_file_fills_the_rest() {
        let home = tempfile::tempdir().expect("tempdir");
        write_private(home.path(), "EXA_API_KEY=a\nTAVILY_API_KEY=b\n");
        let file = CredentialsFile::load(home.path())
            .expect("parses")
            .expect("present");
        let plan = file.plan(|name| name == "EXA_API_KEY");
        assert_eq!(plan.shadowed, vec!["EXA_API_KEY".to_owned()]);
        assert_eq!(plan.applied, vec!["TAVILY_API_KEY".to_owned()]);
    }

    #[test]
    fn a_malformed_line_refuses_the_whole_file_by_line_number() {
        let home = tempfile::tempdir().expect("tempdir");
        write_private(home.path(), "EXA_API_KEY=fine\nthis is not an assignment\n");
        let error = CredentialsFile::load(home.path()).expect_err("malformed");
        assert!(error.contains("line 2"), "{error}");
    }

    #[test]
    fn an_unrendered_gopass_reference_is_refused_by_variable_name() {
        let home = tempfile::tempdir().expect("tempdir");
        write_private(
            home.path(),
            "EXA_API_KEY=gopass:commonmeasure/commonmeasure/exa-api-key\n",
        );
        let error = CredentialsFile::load(home.path()).expect_err("unrendered");
        assert!(error.contains("EXA_API_KEY"), "{error}");
        assert!(error.contains("unrendered"), "{error}");
        assert!(
            !error.contains("exa-api-key"),
            "the reference path is the operator's secret layout and stays out of errors: {error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_file_other_users_can_read_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().expect("tempdir");
        let path = write_private(home.path(), "EXA_API_KEY=a\n");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        let error = CredentialsFile::load(home.path()).expect_err("world-readable");
        assert!(error.contains("chmod 600"), "{error}");
    }

    /// `apply` end to end, with variable names no other test or machine uses
    /// so the process-environment mutation cannot race anything.
    #[test]
    fn apply_sets_absent_variables_and_never_overrides_the_environment() {
        let home = tempfile::tempdir().expect("tempdir");
        write_private(
            home.path(),
            "COMMONMEASURE_CRED_TEST_APPLIED=from-file\n\
             COMMONMEASURE_CRED_TEST_SHADOWED=from-file\n",
        );
        // SAFETY: a name unique to this test; nothing reads it concurrently.
        unsafe { std::env::set_var("COMMONMEASURE_CRED_TEST_SHADOWED", "from-shell") };
        let status = apply(home.path()).expect("applies");
        let loaded = status.loaded.as_ref().expect("present");
        assert_eq!(
            loaded.applied,
            vec!["COMMONMEASURE_CRED_TEST_APPLIED".to_owned()]
        );
        assert_eq!(
            loaded.shadowed,
            vec!["COMMONMEASURE_CRED_TEST_SHADOWED".to_owned()]
        );
        assert_eq!(
            std::env::var("COMMONMEASURE_CRED_TEST_APPLIED").as_deref(),
            Ok("from-file")
        );
        assert_eq!(
            std::env::var("COMMONMEASURE_CRED_TEST_SHADOWED").as_deref(),
            Ok("from-shell"),
            "the environment always wins"
        );
        let recorded = status.to_value().to_string();
        assert!(
            !recorded.contains("from-file"),
            "the record carries names and digest, never a value: {recorded}"
        );
    }
}
