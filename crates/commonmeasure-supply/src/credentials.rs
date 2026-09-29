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

    /// Whether the file sets `name` to a non-empty value.
    pub fn sets(&self, name: &str) -> bool {
        self.value_of(name).is_some()
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
        // `set_var` panics on an empty name, a name holding `=` or NUL, or a
        // value holding NUL, and prints both in the panic. `parse` admits
        // only names of ASCII letters, digits and `_`, and no value holding
        // NUL, so file contents cannot reach that panic.
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

/// Where a provider's variable is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// `credentials.env` sets it and the launching environment does not.
    File,
    /// The launching environment sets it, and wins over the file.
    Environment,
    /// Neither sets it.
    Unset,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Environment => "environment",
            Self::Unset => "unset",
        }
    }
}

/// One implemented provider, the variable it reads and where that variable
/// is set. Names only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderVariable {
    pub provider: &'static str,
    pub variable: &'static str,
    pub origin: Origin,
    /// The file also sets the variable, and the environment wins over it.
    pub shadowed_in_file: bool,
}

/// Every provider in [`crate::IMPLEMENTED_PROVIDERS`], in that order, with
/// its variable and where it is set. The credentials doctor and the
/// console's Sources screen both list this, so they cannot list different
/// providers or name different variables.
///
/// `launch_environment` says whether the launching environment sets a
/// variable, not counting what [`apply`] copied in from the file; `in_file`
/// says whether `credentials.env` sets it.
pub fn provider_variables(
    launch_environment: impl Fn(&str) -> bool,
    in_file: impl Fn(&str) -> bool,
) -> Vec<ProviderVariable> {
    crate::IMPLEMENTED_PROVIDERS
        .iter()
        .filter_map(|&provider| {
            let variable = crate::required_variable(provider)?;
            let environment = launch_environment(variable);
            let file = in_file(variable);
            Some(ProviderVariable {
                provider,
                variable,
                origin: if environment {
                    Origin::Environment
                } else if file {
                    Origin::File
                } else {
                    Origin::Unset
                },
                shadowed_in_file: environment && file,
            })
        })
        .collect()
}

impl CredentialsStatus {
    /// [`provider_variables`] for this process after [`apply`]: a variable
    /// the file supplied is the file's, and any other set variable is the
    /// launching environment's.
    pub fn provider_variables(&self) -> Vec<ProviderVariable> {
        let applied = |name: &str| {
            self.loaded
                .as_ref()
                .is_some_and(|loaded| loaded.applied.iter().any(|entry| entry == name))
        };
        let shadowed = |name: &str| {
            self.loaded
                .as_ref()
                .is_some_and(|loaded| loaded.shadowed.iter().any(|entry| entry == name))
        };
        provider_variables(
            |name| environment_supplies(name) && !applied(name),
            |name| applied(name) || shadowed(name),
        )
    }
}

/// The longest value [`edit_text`] writes. Provider keys are far shorter; the
/// cap keeps a mistaken paste of a document out of the file.
pub const MAX_VALUE_BYTES: usize = 4096;

/// A credentials file's text after one edit.
#[derive(Debug, PartialEq, Eq)]
pub struct Edited {
    pub text: String,
    /// False when the edit left the text as it was.
    pub changed: bool,
}

/// `text` with `name` set to `value`, or with `name` removed when `value` is
/// `None`. Every other line, comment and blank line is kept byte for byte.
///
/// Setting replaces the first line that assigns `name` (with or without
/// `export`, empty placeholders included) and drops any later one, because
/// the last assignment wins and a later line would override the new value;
/// with no such line the assignment is appended. Removing drops every line
/// that assigns `name`.
///
/// The result is parsed as [`CredentialsFile::load`] would parse it, and
/// refused unless it loads and reads `name` back as `value` (or as unset). A
/// refusal never quotes the value or a line of the file.
pub fn edit_text(
    text: &str,
    name: &str,
    value: Option<&str>,
    path: &Path,
) -> Result<Edited, String> {
    let value = match value {
        Some(value) => {
            let value = value.trim();
            if value.is_empty() {
                return Err("the key is empty".to_owned());
            }
            if value.len() > MAX_VALUE_BYTES {
                return Err(format!("the key is longer than {MAX_VALUE_BYTES} bytes"));
            }
            if value.chars().any(char::is_control) {
                return Err("the key contains a line break or another control character".to_owned());
            }
            Some(value)
        }
        None => None,
    };
    parse(text, path).map_err(|reason| format!("{reason}; repair the file first"))?;

    let mut edited = String::with_capacity(text.len() + name.len() + 2);
    let mut written = false;
    for line in text.split_inclusive('\n') {
        if !assigns(line, name) {
            edited.push_str(line);
            continue;
        }
        if let (Some(value), false) = (value, written) {
            let ending = &line[line.trim_end_matches(['\r', '\n']).len()..];
            edited.push_str(&format!("{name}={value}"));
            edited.push_str(if ending.is_empty() { "\n" } else { ending });
            written = true;
        }
    }
    if let (Some(value), false) = (value, written) {
        if !edited.is_empty() && !edited.ends_with('\n') {
            edited.push('\n');
        }
        edited.push_str(&format!("{name}={value}\n"));
    }

    let entries = parse(&edited, path)?;
    let read_back = entries
        .iter()
        .find(|(entry, _)| entry == name)
        .map(|(_, stored)| stored.as_str());
    if read_back != value {
        return Err(match value {
            Some(_) => "the key would not read back as entered; enter it without surrounding \
                        quotes or spaces"
                .to_owned(),
            None => format!("{name} would still be set after removing it"),
        });
    }
    Ok(Edited {
        changed: edited != text,
        text: edited,
    })
}

/// Whether `line` assigns `name`, by the rules [`parse`] reads it with.
fn assigns(line: &str, name: &str) -> bool {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return false;
    }
    let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
    line.split_once('=')
        .is_some_and(|(assigned, _)| assigned.trim() == name)
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
            // The line itself is not quoted: a key pasted without its name is
            // exactly what fails here, and this message reaches terminals,
            // tool errors and the console.
            return Err(format!(
                "{} line {} is not KEY=VALUE",
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
                "{} line {} does not name a variable",
                path.display(),
                index + 1
            ));
        }
        let value = unquote(value.trim());
        if value.contains('\0') {
            // `apply` hands every value to `std::env::set_var`, which panics
            // on a NUL and prints the value in its message. Refusing here, by
            // line number only, keeps the value out of every output.
            return Err(format!(
                "{} line {} contains a NUL character",
                path.display(),
                index + 1
            ));
        }
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

    #[test]
    fn a_malformed_line_is_named_by_number_and_never_quoted() {
        let home = tempfile::tempdir().expect("tempdir");
        write_private(home.path(), "sk-pasted-without-a-name\n=sk-nameless\n");
        let error = CredentialsFile::load(home.path()).expect_err("malformed");
        assert!(!error.contains("sk-pasted"), "{error}");
        write_private(home.path(), "=sk-nameless\n");
        let error = CredentialsFile::load(home.path()).expect_err("nameless");
        assert!(
            error.contains("line 1") && !error.contains("sk-nameless"),
            "{error}"
        );
    }

    /// A NUL in a stored value would panic `std::env::set_var`, which prints
    /// the value. The loader refuses it by line number, and an edit of any
    /// other variable refuses rather than rewrite the file around it.
    #[test]
    fn a_nul_in_a_stored_value_is_refused_by_line_number() {
        let home = tempfile::tempdir().expect("tempdir");
        for content in [
            "# keys\nEXA_API_KEY=sk-nul\0held\n",
            "# keys\nEXA_API_KEY=\"sk-nul\0held\"\n",
            "# keys\nexport EXA_API_KEY=\0sk-nul-held\n",
        ] {
            write_private(home.path(), content);
            let error = CredentialsFile::load(home.path()).expect_err("NUL");
            assert!(error.contains("line 2") && error.contains("NUL"), "{error}");
            assert!(!error.contains("sk-nul"), "{error}");
            let error = apply(home.path()).expect_err("apply refuses before set_var");
            assert!(!error.contains("sk-nul"), "{error}");

            let refused = edit(content, "TAVILY_API_KEY", Some("tv")).expect_err("edit");
            assert!(refused.contains("repair the file first"), "{refused}");
            assert!(!refused.contains("sk-nul"), "{refused}");
        }
    }

    /// The doctor and the console list every implemented provider once, in
    /// the fixed order, and say where each variable is set.
    #[test]
    fn provider_variables_name_every_provider_and_where_its_variable_is_set() {
        let listed = provider_variables(
            |name| name == "EXA_API_KEY" || name == "TAVILY_API_KEY",
            |name| name == "TAVILY_API_KEY" || name == "DATAVILLE_API_KEY",
        );
        assert_eq!(
            listed.iter().map(|row| row.provider).collect::<Vec<_>>(),
            crate::IMPLEMENTED_PROVIDERS.to_vec()
        );
        let row = |provider: &str| {
            listed
                .iter()
                .find(|row| row.provider == provider)
                .expect("listed")
        };
        assert_eq!(row("exa").origin, Origin::Environment);
        assert!(!row("exa").shadowed_in_file);
        assert_eq!(row("tavily").origin, Origin::Environment);
        assert!(row("tavily").shadowed_in_file);
        assert_eq!(row("dataville").origin, Origin::File);
        assert_eq!(row("dataville").variable, "DATAVILLE_API_KEY");
        assert_eq!(row("internal").variable, "COMMONMEASURE_INTERNAL_CORPUS");
        assert_eq!(row("internal").origin, Origin::Unset);
    }

    fn edit(text: &str, name: &str, value: Option<&str>) -> Result<Edited, String> {
        edit_text(text, name, value, Path::new("/home/op/credentials.env"))
    }

    #[test]
    fn setting_a_key_keeps_every_other_line_and_comment() {
        let before = "# operator credentials\n\nexport EXA_API_KEY=old\n# tavily\nTAVILY_API_KEY=t\nEXA_API_KEY=older\n";
        let edited = edit(before, "EXA_API_KEY", Some(" sk-new \n")).expect("edits");
        assert!(edited.changed);
        assert_eq!(
            edited.text,
            "# operator credentials\n\nEXA_API_KEY=sk-new\n# tavily\nTAVILY_API_KEY=t\n"
        );

        let appended = edit("# only a comment", "YOU_API_KEY", Some("y")).expect("appends");
        assert_eq!(appended.text, "# only a comment\nYOU_API_KEY=y\n");
        let created = edit("", "YOU_API_KEY", Some("y")).expect("creates");
        assert_eq!(created.text, "YOU_API_KEY=y\n");
        let placeholder = edit("EXA_API_KEY=\r\nX=1\r\n", "EXA_API_KEY", Some("e")).expect("fills");
        assert_eq!(placeholder.text, "EXA_API_KEY=e\r\nX=1\r\n");
    }

    #[test]
    fn removing_a_key_drops_its_lines_and_nothing_else() {
        let before = "# keep\nEXA_API_KEY=a\nTAVILY_API_KEY=t\nexport EXA_API_KEY=b\n";
        let edited = edit(before, "EXA_API_KEY", None).expect("removes");
        assert_eq!(edited.text, "# keep\nTAVILY_API_KEY=t\n");
        assert!(edited.changed);
        let absent = edit("TAVILY_API_KEY=t\n", "EXA_API_KEY", None).expect("nothing to do");
        assert!(!absent.changed);
    }

    /// A value that would add a line, read back as something else or be
    /// refused at load is refused before anything is written, and the
    /// refusal quotes neither the value nor the file.
    #[test]
    fn an_edit_that_would_not_read_back_is_refused_without_quoting_the_value() {
        for (value, expected) in [
            ("sk-a\nOTHER=planted", "line break"),
            ("", "empty"),
            ("\"sk-quoted\"", "read back"),
            ("gopass:vault/sk-ref", "unrendered"),
        ] {
            let error = edit("", "EXA_API_KEY", Some(value)).expect_err(value);
            assert!(error.contains(expected), "{value:?}: {error}");
            assert!(!error.contains("sk-"), "{error}");
        }
        let long = "k".repeat(MAX_VALUE_BYTES + 1);
        assert!(edit("", "EXA_API_KEY", Some(&long)).is_err());

        let broken = edit("sk-secret-line\n", "EXA_API_KEY", Some("new")).expect_err("broken");
        assert!(broken.contains("repair the file first"), "{broken}");
        assert!(!broken.contains("sk-secret-line"), "{broken}");
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
