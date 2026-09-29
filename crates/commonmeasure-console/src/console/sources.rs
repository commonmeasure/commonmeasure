//! The Sources screen's data and its one write: a provider key in the
//! operator credentials file, `<home>/credentials.env`.
//!
//! The projection names each provider's variable and where it is set, never
//! a value. It re-reads the file on every call, so a key saved here shows at
//! once; the launching environment is the one the console started under,
//! because `credentials::apply` copied the file into this process at start
//! and the environment cannot tell the two apart after that.
//!
//! A write changes one variable and keeps every other line, holds a lock
//! against a concurrent write, is refused unless the result loads as the
//! mediator will load it, replaces the file atomically at mode 0600, and
//! appends `credentials_changed` to [`CHANGES_LOG`] naming the variable and
//! the new file digest. The value reaches only the file: no answer, notice,
//! log line or record carries it.
//!
//! An edge whose hosted service takes supplier keys from its Hub (supplier
//! custody) is refused, as a managed policy is (`super::edit::local_policy`):
//! a key in the file would override the organisation's.

use std::path::Path;

use commonmeasure_harness::managed::Deployment;
use commonmeasure_runtime::declaration::{self, LockRefused};
use commonmeasure_runtime::evidence::EvidenceLog;
use commonmeasure_supply::credentials::{self, CREDENTIALS_FILE};
use commonmeasure_types::canonical::sha256_digest;
use serde_json::{Value, json};

use super::edit::Outcome;

/// Where the console records each change it makes to the credentials file.
/// Outside `sessions/`, so the relay never reads it.
pub const CHANGES_LOG: &str = "credentials-changes.ndjson";

/// Held for the read, edit and rename of one write.
const LOCK_FILE: &str = "credentials.lock";

/// What a saved change reaches, stated on every save.
const REACH: &str = "Sessions that start from now read it. Running sessions, and Compare on this \
                     console, keep the keys they started with.";

/// Who supplies this edge's supplier keys, and so whether the page may write
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Custody {
    /// The operator: the file and the launching environment.
    Local,
    /// The organisation's Hub releases them to this edge's hosted service.
    Organisation,
    /// The declarations that decide could not be read; nothing is written.
    Unknown(String),
}

/// Read who supplies supplier keys. A local edge, and a managed edge whose
/// hosted service does not set `supplier_custody`, keep their own keys.
pub fn custody(home: &Path) -> Custody {
    match Deployment::read(home) {
        Ok(Deployment::Local) => return Custody::Local,
        Ok(Deployment::Managed { .. }) => {}
        Err(error) => {
            return Custody::Unknown(format!(
                "Key editing unavailable: repair deployment.json. {error}"
            ));
        }
    }
    let path = home.join(commonmeasure_harness::delivery::SERVICE_CONFIG_FILE);
    let encoded = match std::fs::read(&path) {
        Ok(encoded) => encoded,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Custody::Local,
        Err(error) => {
            return Custody::Unknown(format!(
                "Key editing unavailable: {} cannot be read. {error}",
                path.display()
            ));
        }
    };
    // Read as the hosted service's typed loader reads it: an object whose
    // `supplier_custody`, when present, is a boolean. Any other shape is a
    // file the service refuses to start on, so who holds the keys is unknown.
    let config = match serde_json::from_slice::<Value>(&encoded) {
        Ok(config) => config,
        Err(error) => {
            return Custody::Unknown(format!(
                "Key editing unavailable: {} is not JSON. {error}",
                path.display()
            ));
        }
    };
    let Some(config) = config.as_object() else {
        return Custody::Unknown(format!(
            "Key editing unavailable: {} is not a JSON object.",
            path.display()
        ));
    };
    match config.get("supplier_custody") {
        None | Some(Value::Bool(false)) => Custody::Local,
        Some(Value::Bool(true)) => Custody::Organisation,
        Some(_) => Custody::Unknown(format!(
            "Key editing unavailable: supplier_custody in {} is not true or false.",
            path.display()
        )),
    }
}

/// Whether `provider` takes a key the page may write. The corpus root and a
/// skill catalogue are paths on this machine, set in the file by hand.
fn takes_key(provider: &str) -> bool {
    commonmeasure_supply::remote_adapter(provider, None, "").is_some()
}

/// The Sources screen's values. `launch_environment` names the provider
/// variables the launching environment set when the console started.
pub fn projection(home: &Path, launch_environment: &[String]) -> Value {
    let path = home.join(CREDENTIALS_FILE);
    let (file, file_error) = match credentials::CredentialsFile::load(home) {
        Ok(file) => (file, None),
        Err(error) => (None, Some(error)),
    };
    let custody = custody(home);
    let rows: Vec<Value> = credentials::provider_variables(
        |name| launch_environment.iter().any(|set| set == name),
        |name| file.as_ref().is_some_and(|file| file.sets(name)),
    )
    .into_iter()
    .map(|row| {
        json!({
            "name": row.provider,
            "variable": row.variable,
            "origin": row.origin.as_str(),
            "shadowed_in_file": row.shadowed_in_file,
            "takes_key": takes_key(row.provider),
        })
    })
    .collect();
    json!({
        "file": {
            "path": path.display().to_string(),
            "present": path.exists(),
            "error": file_error,
        },
        "custody": match &custody {
            Custody::Local => "local",
            Custody::Organisation => "organisation",
            Custody::Unknown(_) => "unknown",
        },
        "edit_error": match &custody {
            Custody::Unknown(reason) => Some(reason.as_str()),
            _ => None,
        },
        "providers": rows,
    })
}

/// Set (`key` is `Some`) or remove one provider's key in `credentials.env`.
pub fn write_key(
    home: &Path,
    provider: &str,
    key: Option<&str>,
    launch_environment: &[String],
) -> Outcome {
    match custody(home) {
        Custody::Local => {}
        Custody::Organisation => {
            return Outcome::Refused {
                status: 403,
                notice: "Not saved: supplier keys on this edge are managed by your \
                         organisation's Hub."
                    .into(),
            };
        }
        Custody::Unknown(reason) => {
            return Outcome::Refused {
                status: 403,
                notice: reason,
            };
        }
    }
    let variable = match commonmeasure_supply::required_variable(provider) {
        Some(variable) if takes_key(provider) => variable,
        Some(variable) => {
            return Outcome::Refused {
                status: 400,
                notice: format!(
                    "Not saved: {variable} names a path on this machine. Set it in \
                     credentials.env by hand."
                ),
            };
        }
        // The field is not quoted: a key pasted into the wrong field would
        // otherwise come back in the notice.
        None => {
            return Outcome::Refused {
                status: 400,
                notice: "Not saved: the form named no provider this edge implements.".into(),
            };
        }
    };
    let _lock = match declaration::lock(&home.join(LOCK_FILE)) {
        Ok(lock) => lock,
        Err(LockRefused::Busy(reason)) => {
            return Outcome::Refused {
                status: 409,
                notice: format!("Not saved: {reason}."),
            };
        }
        Err(refused @ (LockRefused::LockFile(_) | LockRefused::Failed(_))) => {
            return Outcome::Refused {
                status: 500,
                notice: format!("Not saved: {refused}."),
            };
        }
    };
    let path = home.join(CREDENTIALS_FILE);
    let (before, previous_mode) = match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Outcome::Refused {
                status: 409,
                notice: format!(
                    "Not saved: {} is a symbolic link, and saving here would replace the link \
                     with a file. Edit the file it points to by hand.",
                    path.display()
                ),
            };
        }
        Ok(metadata) => match std::fs::read(&path) {
            Ok(bytes) => (Some(bytes), mode_of(&metadata)),
            Err(error) => return unreadable(&path, &error),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (None, None),
        Err(error) => return unreadable(&path, &error),
    };
    let Ok(text) = String::from_utf8(before.clone().unwrap_or_default()) else {
        return Outcome::Refused {
            status: 409,
            notice: format!(
                "Not saved: {} is not UTF-8. Repair it by hand.",
                path.display()
            ),
        };
    };
    let edited = match credentials::edit_text(&text, variable, key, &path) {
        Ok(edited) => edited,
        Err(reason) => {
            return Outcome::Refused {
                status: 400,
                notice: format!("Not saved: {reason}."),
            };
        }
    };
    // Removing what is not there changes nothing. Setting always writes,
    // even the same value: an answer that said a submitted key matched the
    // stored one would let a page test guesses against it.
    if key.is_none() && !edited.changed {
        return Outcome::Unchanged(format!(
            "{variable} is not in {}. Nothing was written.",
            path.display()
        ));
    }
    if let Err(reason) = declaration::replace_private(&path, edited.text.as_bytes()) {
        return Outcome::Refused {
            status: 500,
            notice: format!("Not saved: {reason}"),
        };
    }

    let sha256 = sha256_digest(edited.text.as_bytes());
    let mut notice = match key {
        Some(_) => format!("Saved {variable} to {}. {REACH}", path.display()),
        None => format!(
            "Removed {variable} from {}. Sessions that start from now run without it; running \
             sessions keep it until they restart.",
            path.display()
        ),
    };
    if launch_environment.iter().any(|set| set == variable) {
        notice.push_str(&format!(
            " The environment this console started in also sets {variable}; a session started \
             with it set uses that value instead."
        ));
    }
    if previous_mode.is_some_and(|mode| mode & 0o077 != 0) {
        notice.push_str(" The file was readable by other users; it is now owner-only (600).");
    }
    let record = json!({
        "path": path.display().to_string(),
        "action": if key.is_some() { "set" } else { "removed" },
        "provider": provider,
        "variable": variable,
        "sha256": sha256,
        "previous_sha256": before.as_deref().map(sha256_digest),
        "written_by": "console",
    });
    let log = home.join(CHANGES_LOG);
    if let Err(error) =
        EvidenceLog::open_append(&log).and_then(|mut log| log.append("credentials_changed", record))
    {
        notice.push_str(&format!(
            " The change was not recorded in {}: {error}.",
            log.display()
        ));
    }
    Outcome::Saved(notice)
}

fn unreadable(path: &Path, error: &std::io::Error) -> Outcome {
    Outcome::Refused {
        status: 500,
        notice: format!("Not saved: {} cannot be read. {error}", path.display()),
    }
}

#[cfg(unix)]
fn mode_of(metadata: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt as _;
    Some(metadata.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn mode_of(_metadata: &std::fs::Metadata) -> Option<u32> {
    None
}
