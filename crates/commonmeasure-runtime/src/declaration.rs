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
    replace_as(target, bytes, Access::Default)
}

/// [`replace`] for a file that holds a credential: the temporary file is
/// created readable by the owner only, where the platform has modes, so the
/// secret is never on disk under a wider mode, not even before a rename.
pub fn replace_private(target: &Path, bytes: &[u8]) -> Result<(), String> {
    replace_as(target, bytes, Access::Owner)
}

#[derive(Clone, Copy)]
enum Access {
    Default,
    Owner,
}

fn replace_as(target: &Path, bytes: &[u8], access: Access) -> Result<(), String> {
    let directory = target
        .parent()
        .ok_or_else(|| format!("{} has no parent directory to write into", target.display()))?;
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "declaration".to_owned());
    let (temp, file) = create_unique_temp(directory, &name, access)?;
    if let Err(error) = write_durably(file, &temp, bytes) {
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
///
/// The lock file is opened by [`open_lock`], so one this call creates is
/// readable by its owner only.
pub fn lock(path: &Path) -> Result<Lock, LockRefused> {
    lock_within(path, LOCK_DEADLINE)
}

/// [`lock`] with a wait of the caller's own, for a lock whose holders keep
/// it for a different span than an editor's save. The busy refusal says no
/// edit was made; a caller whose change is not an edit words its own.
pub fn lock_within(path: &Path, wait: std::time::Duration) -> Result<Lock, LockRefused> {
    let file = open_lock(path).map_err(|error| refused_open(path, error))?;
    let deadline = std::time::Instant::now() + wait;
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
                wait.as_secs()
            )));
        }
        std::thread::sleep(LOCK_RETRY);
    }
}

/// Why [`open_lock`] failed, by what the operator can do about it. Denied
/// on a file that exists, the fault is that file's mode or owner, such as a
/// lock file a `sudo` run created: its directory can be writable and the
/// file still refuse this user. Anything else, a lock file that cannot be
/// created among them, is the directory's.
fn refused_open(path: &Path, error: std::io::Error) -> LockRefused {
    let cause = format!("open lock {}: {error}", path.display());
    if error.kind() == std::io::ErrorKind::PermissionDenied && path.symlink_metadata().is_ok() {
        LockRefused::LockFile(cause)
    } else {
        LockRefused::Failed(cause)
    }
}

/// Open the lock file at `path` for reading and writing, creating it if
/// absent. Every lock file in the home is created here, and every holder
/// opens its lock file here, including those that wait differently from
/// [`lock`]. The one other open of a lock file is the harness's
/// `delivery::service_running`, a read-only probe that creates nothing.
///
/// A lock file this call creates is readable by its owner only, where the
/// platform has modes: `flock` needs only a read descriptor, so a lock file
/// another local user can open is one they can hold, and every change
/// behind it is then refused as busy. The home is not always 0700 (it is
/// made under the umask, or named by the operator), so the lock file cannot
/// rely on it. A lock file that exists already keeps its mode.
pub fn open_lock(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

/// How long a save waits for another writer before saying so. A competing
/// console holds a lock across one revision check and one rename, so any wait
/// longer than this is a process that is not going to let go.
pub const LOCK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);
const LOCK_RETRY: std::time::Duration = std::time::Duration::from_millis(20);

/// An exclusive hold on one declaration. Released when dropped, and by the
/// kernel if this process dies, so a crash mid-save cannot wedge an editor.
/// `flock` on unix, `LockFileEx` on Windows — the console ships on both, and a
/// target where this held nothing would lose a concurrent edit silently.
#[derive(Debug)]
pub struct Lock {
    _file: std::fs::File,
}

/// Why a lock was not taken. Each has its own remedy, so a caller that
/// names one does not name another's: waiting helps a busy lock, and a
/// writable directory helps neither a busy lock nor a lock file that exists
/// and refuses this user.
#[derive(Debug)]
pub enum LockRefused {
    /// Another process held the lock for the whole wait.
    Busy(String),
    /// The lock file exists and this user cannot open it for reading and
    /// writing. The text is the cause; the display adds
    /// [`LOCK_FILE_REMEDY`].
    LockFile(String),
    /// Any other fault: the lock file could not be created, or the lock
    /// call itself failed.
    Failed(String),
}

/// What to do about a lock file that exists and refuses this user. Removing
/// one that a process holds would let a second holder in beside it.
pub const LOCK_FILE_REMEDY: &str = "make that lock file readable and writable by this user, or remove it while no process holds it";

impl std::fmt::Display for LockRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy(reason) | Self::Failed(reason) => write!(f, "{reason}"),
            Self::LockFile(cause) => write!(f, "{cause}; {LOCK_FILE_REMEDY}"),
        }
    }
}

/// Write bytes and flush them to the disk itself. Without the flush the rename
/// can land before the data, and a crash leaves a zero-length declaration: the
/// load reports it and the console offers the repair editor, but the
/// operator's rules or policy are gone.
fn write_durably(mut file: std::fs::File, path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    file.write_all(bytes)
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    file.sync_all()
        .map_err(|error| format!("flush {}: {error}", path.display()))
}

/// How many temporary names a save tries before it gives up. One taken name
/// is a crashed writer's leftover under a reused pid; eight in a row is a
/// directory something else is filling, and the operator needs to hear so.
const TEMP_ATTEMPTS: usize = 8;

/// Create a temporary file this call owns, passing over names that are taken.
/// A taken name belongs to another writer, live or crashed, so it is neither
/// written into nor removed: the caller unlinks only the path returned here.
fn create_unique_temp(
    directory: &Path,
    name: &str,
    access: Access,
) -> Result<(std::path::PathBuf, std::fs::File), String> {
    for _ in 0..TEMP_ATTEMPTS {
        let temp = temp_path(directory, name);
        match create_temp(&temp, access) {
            Ok(file) => return Ok((temp, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(format!("write {}: {error}", temp.display())),
        }
    }
    Err(format!(
        "no temporary name for {name} was free in {} after {TEMP_ATTEMPTS} tries; \
         remove leftover {name}.*.tmp files there if no save is running",
        directory.display()
    ))
}

/// Create the writer's temporary file. The name is the writer's own, so the
/// file must not exist: `create_new` refuses rather than writing into a file
/// someone else made, whose mode would be theirs. The mode is set in the
/// same call that creates the file.
fn create_temp(path: &Path, access: Access) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if let Access::Owner = access {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = access;
    options.open(path)
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

fn temp_path(directory: &Path, name: &str) -> std::path::PathBuf {
    #[cfg(test)]
    if let Some(planned) = tests::planned_temp() {
        return directory.join(planned);
    }
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    directory.join(format!(
        "{name}.{}.{}.{}.tmp",
        std::process::id(),
        since.as_secs(),
        temp_nonce(since.subsec_nanos()),
    ))
}

/// Enough to separate two writers' temp files. The pid separates processes;
/// the counter separates saves within one; the seconds and nanoseconds make
/// a reused pid unlikely to draw a crashed writer's name, and
/// [`create_unique_temp`] passes over one that it does draw.
fn temp_nonce(subsec_nanos: u32) -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    u64::from(subsec_nanos) ^ (COUNTER.fetch_add(1, Ordering::Relaxed) << 32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    thread_local! {
        /// Temporary names the next saves on this thread take, in order, so
        /// a test can put a file where the writer is going to look.
        static PLANNED_TEMPS: RefCell<VecDeque<String>> = const { RefCell::new(VecDeque::new()) };
    }

    pub(super) fn planned_temp() -> Option<String> {
        PLANNED_TEMPS.with(|planned| planned.borrow_mut().pop_front())
    }

    /// EGR-59. A crash leaves a temporary behind, and a later process with
    /// the same pid draws the same name. The save takes another name rather
    /// than failing, and leaves the orphan alone: it is not this call's file.
    #[test]
    fn a_temporary_name_already_taken_is_passed_over_and_left_alone() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("policy.json");
        let orphan = home.path().join("policy.json.orphan.tmp");
        std::fs::write(&orphan, b"an earlier process's bytes").unwrap();
        PLANNED_TEMPS.with(|planned| {
            planned.borrow_mut().extend([
                "policy.json.orphan.tmp".to_owned(),
                "policy.json.fresh.tmp".to_owned(),
            ])
        });

        let saved = replace(&target, b"{}");
        PLANNED_TEMPS.with(|planned| planned.borrow_mut().clear());
        assert_eq!(
            std::fs::read(&orphan).ok().as_deref(),
            Some(&b"an earlier process's bytes"[..]),
            "the orphan is not this call's to remove or overwrite"
        );
        saved.unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"{}");
        assert!(!home.path().join("policy.json.fresh.tmp").exists());
    }

    /// Every name taken: the save stops after a bounded number of tries,
    /// says so, and leaves the target and every taken name as they were.
    #[test]
    fn a_save_that_finds_no_free_temporary_name_says_so_and_touches_nothing() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("policy.json");
        std::fs::write(&target, b"old").unwrap();
        let taken: Vec<String> = (0..TEMP_ATTEMPTS)
            .map(|n| format!("policy.json.taken{n}.tmp"))
            .collect();
        for name in &taken {
            std::fs::write(home.path().join(name), name).unwrap();
        }
        PLANNED_TEMPS.with(|planned| planned.borrow_mut().extend(taken.iter().cloned()));

        let refused = replace(&target, b"new");
        PLANNED_TEMPS.with(|planned| planned.borrow_mut().clear());
        let refused = refused.unwrap_err();
        assert!(
            refused.contains(&format!("after {TEMP_ATTEMPTS} tries")),
            "{refused}"
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        for name in &taken {
            assert_eq!(
                std::fs::read_to_string(home.path().join(name)).unwrap(),
                *name
            );
        }
    }

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

    /// A lock file is created readable by its owner only, so another local
    /// user who can reach the home cannot open it to hold it. One that
    /// exists already keeps its mode.
    #[cfg(unix)]
    #[test]
    fn a_created_lock_file_is_owner_only_and_an_existing_one_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt as _;
        let home = tempfile::tempdir().unwrap();
        let created = home.path().join("policy.lock");
        drop(lock(&created).unwrap());
        assert_eq!(
            std::fs::metadata(&created).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let existing = home.path().join("attribution.lock");
        std::fs::write(&existing, b"").unwrap();
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o644)).unwrap();
        drop(lock(&existing).unwrap());
        assert_eq!(
            std::fs::metadata(&existing).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    /// The owner-only assertions here pass under a umask that already masks
    /// 066, such as 077, whatever mode `open_lock` asks for. This test runs
    /// its body again in a child process of this test binary whose umask is
    /// 022, set between fork and exec, so neither this process's umask nor
    /// its other tests are touched and the check holds under any umask the
    /// suite runs with (writes-console-p3 review P3-3).
    #[cfg(unix)]
    #[test]
    fn a_created_lock_file_is_owner_only_under_a_permissive_umask() {
        use std::os::unix::fs::PermissionsExt as _;
        use std::os::unix::process::CommandExt as _;
        const CHILD_HOME: &str = "COMMONMEASURE_TEST_UMASK_CHILD_HOME";
        const NAME: &str =
            "declaration::tests::a_created_lock_file_is_owner_only_under_a_permissive_umask";
        if let Some(home) = std::env::var_os(CHILD_HOME) {
            drop(open_lock(&Path::new(&home).join("policy.lock")).unwrap());
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", NAME, "--test-threads=1"])
            .env(CHILD_HOME, home.path());
        // SAFETY: `umask` is async-signal-safe and changes only the child's
        // own process state, between fork and exec.
        unsafe {
            child.pre_exec(|| {
                libc::umask(0o022);
                Ok(())
            });
        }
        let output = child.output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{stdout}");
        assert!(
            stdout.contains("1 passed"),
            "the child ran no test: {stdout}"
        );
        let created = home.path().join("policy.lock");
        assert_eq!(
            std::fs::metadata(&created).unwrap().permissions().mode() & 0o777,
            0o600,
            "created under umask 022"
        );
    }

    /// A credential's temporary file is owner-only from the call that
    /// creates it, before a byte is written, and the file that lands keeps
    /// that mode over one that was wider.
    #[cfg(unix)]
    #[test]
    fn a_private_replacement_is_owner_only_from_creation() {
        use std::os::unix::fs::PermissionsExt as _;
        let home = tempfile::tempdir().unwrap();
        let temp = home.path().join("key.json.1.2.tmp");
        let created = create_temp(&temp, Access::Owner).unwrap();
        assert_eq!(created.metadata().unwrap().len(), 0);
        assert_eq!(
            created.metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(
            create_temp(&temp, Access::Owner).is_err(),
            "an existing file is not written into"
        );

        let target = home.path().join("key.json");
        std::fs::write(&target, b"old").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        replace_private(&target, b"secret").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"secret");
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
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

    /// EGR-119. A lock file that exists and that this user cannot open is a
    /// fault of that file, not of its directory and not of another holder:
    /// the refusal names the file and says what to do with it.
    #[cfg(unix)]
    #[test]
    fn a_lock_file_this_user_cannot_open_is_refused_as_the_files_fault() {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        let home = tempfile::tempdir().unwrap();
        // Root opens a file whatever its mode.
        if std::fs::metadata(home.path()).unwrap().uid() == 0 {
            return;
        }
        let path = home.path().join("policy.lock");
        std::fs::write(&path, b"").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let refusal = lock(&path).unwrap_err();
        let text = refusal.to_string();
        assert!(
            !matches!(refusal, LockRefused::Busy(_) | LockRefused::Failed(_)),
            "{text}"
        );
        assert!(text.contains(&path.display().to_string()), "{text}");
        assert!(
            text.ends_with("make that lock file readable and writable by this user, or remove it while no process holds it"),
            "{text}"
        );
    }
}
