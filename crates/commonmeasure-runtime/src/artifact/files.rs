//! Bounded files and complete, exclusive publication of immutable records.
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

pub(super) fn checked_path(path: &Path, create: bool) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() {
        return Err("empty path".into());
    }
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    let mut current = PathBuf::new();
    for part in full.components() {
        match part {
            Component::ParentDir => return Err("parent traversal is not supported".into()),
            Component::CurDir => continue,
            _ => current.push(part),
        }
        let metadata = match fs::symlink_metadata(&current) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
                match fs::create_dir(&current) {
                    Ok(()) => {
                        if let Some(parent) = current.parent() {
                            File::open(parent)
                                .and_then(|f| f.sync_all())
                                .map_err(|e| e.to_string())?;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(format!("create {}: {e}", current.display())),
                }
                fs::symlink_metadata(&current).map_err(|e| e.to_string())?
            }
            Err(e) => return Err(format!("inspect {}: {e}", current.display())),
        };
        if metadata.file_type().is_symlink() {
            return Err(format!("symlink is not supported: {}", current.display()));
        }
        if create && !metadata.is_dir() {
            return Err(format!("not a directory: {}", current.display()));
        }
    }
    Ok(current)
}

pub(super) fn read(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let path = checked_path(path, false)?;
    let before = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    if !before.is_file() || before.len() > limit as u64 {
        return Err(format!(
            "{} is not a regular file within the {limit}-byte limit",
            path.display()
        ));
    }
    let mut file = File::open(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let opened = file.metadata().map_err(|e| e.to_string())?;
    if !opened.is_file() || opened.len() > limit as u64 {
        return Err("opened file exceeds its size/type boundary".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != opened.dev() || before.ino() != opened.ino() {
            return Err("file changed while being opened".into());
        }
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("file grew beyond the byte limit".into());
    }
    let after = file.metadata().map_err(|e| e.to_string())?;
    let named = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    if !named.is_file()
        || opened.len() != after.len()
        || bytes.len() as u64 != opened.len()
        || opened.modified().map_err(|e| e.to_string())?
            != after.modified().map_err(|e| e.to_string())?
    {
        return Err("file changed during capture; retry after saving finishes".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // Publication removes its temporary hard-link after the final name is
        // visible. That changes ctime without changing the published bytes.
        let changed_ctime =
            opened.ctime() != after.ctime() || opened.ctime_nsec() != after.ctime_nsec();
        if opened.dev() != named.dev()
            || opened.ino() != named.ino()
            || (changed_ctime && opened.nlink() == after.nlink())
        {
            return Err("file changed during capture; retry after saving finishes".into());
        }
    }
    Ok(bytes)
}

/// A hard-link publishes a fully synced inode only if the destination is absent.
/// Unlike rename, this cannot replace an immutable record written by a peer.
pub(super) fn publish(path: &Path, bytes: &[u8]) -> Result<bool, String> {
    let parent = path.parent().ok_or("publication has no parent")?;
    let parent = checked_path(parent, false)?;
    let temp = parent.join(format!(".pending-{}", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        match fs::hard_link(&temp, path) {
            Ok(()) => {
                File::open(&parent)
                    .and_then(|f| f.sync_all())
                    .map_err(|e| e.to_string())?;
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(format!("publish {}: {e}", path.display())),
        }
    })();
    let cleanup = fs::remove_file(&temp);
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), _) => Err(error),
        (_, Err(error)) => Err(format!("remove publication temporary: {error}")),
    }
}
