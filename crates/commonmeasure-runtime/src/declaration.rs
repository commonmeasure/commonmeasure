//! Writing the operator's declared artefacts, one implementation.
//!
//! The console edits declarations — the attribution rules (`commonmeasure-console`) and the
//! session policy (this crate) — and both writes have to hold the same
//! properties, because both replace a file another process is reading:
//!
//! - **atomic**: bytes into a uniquely named temp file, flushed, then renamed,
//!   so a reader sees the old declaration or the new one and never a half of
//!   either, and a rejected save never touches the target at all;
//! - **revision-checked**: a token over the bytes an editor was rendered from,
//!   so a save against a file that changed underneath can be refused with both
//!   versions stated rather than taken silently;
//! - **exclusive across processes**: two consoles on one home is an ordinary
//!   way to run this, and an in-process mutex cannot see the second one.
//!
//! They lived twice, once per artefact, until the policy editor needed the
//! third copy. A second write path is exactly where the property quietly
//! weakens — the durability flush dropped, the temp name shared again — so
//! there is one, and each artefact keeps only what is genuinely its own: its
//! file name, its validity rule and its errors.

use std::path::Path;

/// The revision of a declaration that does not exist yet. A save stating this
/// back is checked like any other: if a file has appeared meanwhile, the token
/// no longer matches and the save is refused rather than clobbering it.
pub const ABSENT: &str = "absent";

/// A token over the bytes a declaration was read from.
pub fn revision_of(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

/// The current revision of `path`, or [`ABSENT`] when there is no file.
pub fn revision(path: &Path) -> Result<String, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(revision_of(&bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ABSENT.to_owned()),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

/// Replace `target` with `bytes`, atomically and durably.
///
/// The temp name is unique to the writer: a shared one made two consoles on
/// one home rename each other's file away, which surfaced to the operator as a
/// raw ENOENT instead of the stated conflict.
pub fn replace(target: &Path, bytes: &[u8]) -> Result<(), String> {
    let directory = target
        .parent()
        .ok_or_else(|| format!("{} has no parent directory to write into", target.display()))?;
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "declaration".to_owned());
    let temp = directory.join(format!(
        "{name}.{}.{}.tmp",
        std::process::id(),
        temp_nonce(),
    ));
    if let Err(error) = write_durably(&temp, bytes) {
        std::fs::remove_file(&temp).ok();
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&temp, target) {
        std::fs::remove_file(&temp).ok();
        return Err(format!("rename {} into place: {error}", target.display()));
    }
    sync_dir(directory)
}

/// Hold `path` exclusively, across processes, for as long as the returned
/// guard lives, or refuse within [`LOCK_DEADLINE`].
///
/// The check-and-rename step has to be one step against *any* writer, or a
/// concurrent edit is lost silently rather than refused with both versions
/// stated. The wait is bounded because this runs on a served request: an
/// unbounded one parks the handler for as long as another process holds the
/// lock, and enough parked handlers exhaust the server's connections — a
/// console that answers nothing at all, over a file nobody is editing.
pub fn lock(path: &Path) -> Result<Lock, LockRefused> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .map_err(|error| LockRefused::Failed(format!("open lock {}: {error}", path.display())))?;
    let deadline = std::time::Instant::now() + LOCK_DEADLINE;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Lock { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(LockRefused::Failed(format!(
                    "lock {}: {error}",
                    path.display()
                )));
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(LockRefused::Busy(format!(
                "another process has held {} for {}s; no edit was made",
                path.display(),
                LOCK_DEADLINE.as_secs()
            )));
        }
        std::thread::sleep(LOCK_RETRY);
    }
}

/// How long a save waits for another writer before saying so. A competing
/// console holds a lock across one revision check and one rename, so any wait
/// longer than this is a process that is not going to let go.
const LOCK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);
const LOCK_RETRY: std::time::Duration = std::time::Duration::from_millis(20);

/// An exclusive hold on one declaration. Released when dropped, and by the
/// kernel if this process dies, so a crash mid-save cannot wedge an editor.
/// `flock` on unix, `LockFileEx` on Windows — the console ships on both, and a
/// target where this held nothing would lose a concurrent edit silently.
#[derive(Debug)]
pub struct Lock {
    _file: std::fs::File,
}

/// Why a lock was not taken. A busy home and a broken one are different
/// answers: the first is a wait that ended, the second is a fault.
#[derive(Debug)]
pub enum LockRefused {
    Busy(String),
    Failed(String),
}

impl std::fmt::Display for LockRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy(reason) | Self::Failed(reason) => write!(f, "{reason}"),
        }
    }
}

/// Write bytes and flush them to the disk itself. Without the flush the rename
/// can land before the data, and a crash leaves a zero-length declaration: the
/// load reports it and the console offers the repair editor, but the
/// operator's rules or policy are gone.
fn write_durably(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    let mut file = std::fs::File::create(path)
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    file.write_all(bytes)
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    file.sync_all()
        .map_err(|error| format!("flush {}: {error}", path.display()))
}

/// Persist the rename itself, not only the bytes it moved. Unix only: there is
/// no portable way to flush a directory entry, so on other targets the rename
/// is as durable as the platform makes it and no more. What a reader sees is
/// unaffected either way; only a crash in the seconds after a save can tell
/// the difference.
#[cfg(unix)]
fn sync_dir(directory: &Path) -> Result<(), String> {
    std::fs::File::open(directory)
        .and_then(|dir| dir.sync_all())
        .map_err(|error| format!("flush directory {}: {error}", directory.display()))
}

#[cfg(not(unix))]
fn sync_dir(_directory: &Path) -> Result<(), String> {
    Ok(())
}

/// Enough to separate two writers' temp files. The pid already separates
/// processes; this separates saves within one, and the clock separates a
/// reused pid.
fn temp_nonce() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let ticks = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| u64::from(since.subsec_nanos()))
        .unwrap_or(0);
    ticks ^ (COUNTER.fetch_add(1, Ordering::Relaxed) << 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replaced_file_holds_the_new_bytes_and_leaves_no_temp_behind() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("policy.json");
        replace(&target, b"{}").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"{}");
        replace(&target, b"{\"policy_mode\":\"strict\"}").unwrap();
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "{\"policy_mode\":\"strict\"}"
        );

        let leftovers: Vec<String> = std::fs::read_dir(home.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }

    #[test]
    fn revision_names_an_absent_file_and_tracks_the_bytes() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("policy.json");
        assert_eq!(revision(&target).unwrap(), ABSENT);
        replace(&target, b"one").unwrap();
        let first = revision(&target).unwrap();
        assert_ne!(first, ABSENT);
        replace(&target, b"two").unwrap();
        assert_ne!(revision(&target).unwrap(), first);
        assert_eq!(revision(&target).unwrap(), revision_of(b"two"));
    }

    /// Held elsewhere, the lock refuses with the wait named rather than parking
    /// the handler that took it.
    #[test]
    fn a_lock_another_holder_keeps_is_refused_within_the_deadline() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("policy.lock");
        let held = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .unwrap();
        held.lock().unwrap();

        let started = std::time::Instant::now();
        let refusal = lock(&path).unwrap_err();
        assert!(
            matches!(refusal, LockRefused::Busy(_)),
            "a held lock is busy, not broken: {refusal}"
        );
        assert!(
            refusal.to_string().contains("no edit was made"),
            "the refusal says what did not happen: {refusal}"
        );
        assert!(
            started.elapsed() < LOCK_DEADLINE * 4,
            "the wait ran past any deadline"
        );

        drop(held);
        assert!(lock(&path).is_ok(), "released, the next attempt takes it");
    }

    /// Two writers' temp files must not share a name, or one renames the
    /// other's file away mid-save.
    #[test]
    fn concurrent_replacements_land_whole() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("policy.json");
        std::thread::scope(|scope| {
            for n in 0..8 {
                let target = target.clone();
                scope.spawn(move || {
                    let body = format!("{{\"n\":{n}}}");
                    replace(&target, body.as_bytes()).unwrap();
                });
            }
        });
        let landed = std::fs::read_to_string(&target).unwrap();
        assert!(
            landed.starts_with("{\"n\":") && landed.ends_with('}'),
            "one writer's bytes, entire: {landed}"
        );
    }
}
