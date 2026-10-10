//! Bodies a mediated fetch hands over as a file rather than as text
//! (EGR-112; `docs/contracts/host-integration.md` §A fetched file,
//! `docs/FAIL-POLICY.md` §6).
//!
//! A PDF is acquired through the edge like any page: host policy, robots,
//! licence, pacing, the private-address floor, redirects and the crossing
//! all apply. Its bytes are never decoded as text here. A local edge saves
//! them under the session's directory and names the file; a hosted edge,
//! which shares no file system with the harness, returns them as an MCP
//! embedded resource. The harness's own tools parse the file.
//!
//! Which bodies are text: every body is, as the extractor decodes it
//! (`commonmeasure_runtime::processor::extract`), except a PDF and the
//! media types [`classify`] names as other files. A body labelled with any
//! other type, or with none, is decoded as text as it always was.

use std::path::{Path, PathBuf};

use commonmeasure_types::canonical::sha256_digest;

/// The largest file one `context_fetch` hands over, in bytes: the HTTP
/// client's body ceiling, which stops the transfer at this size rather than
/// reading on ([`commonmeasure_http::MAX_BODY_BYTES`]).
pub const MAX_FILE_BYTES: usize = commonmeasure_http::MAX_BODY_BYTES;

/// The media type a PDF is delivered under, whatever the origin labelled it.
pub const PDF: &str = "application/pdf";

/// What a delivered file's crossing says of it, word for word: the edge
/// handed the bytes over and read none of them, so the record claims
/// nothing about what the model read, and the screens that rule on text
/// gave no verdict.
pub const NOT_READ: &str = "delivered as a file, not read by the edge; the PII detector and the \
                             injection screen did not run over it";

/// What a fetched body is, for delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyKind {
    /// Decoded and delivered as text.
    Text,
    /// Delivered whole as a file.
    Pdf,
    /// A file this edge does not deliver: an explicit unavailable result
    /// naming the media type, and no bytes kept.
    Unsupported(String),
}

/// Decide how a body is delivered from its `Content-Type` and its first
/// bytes. A body starting `%PDF-` is a PDF whatever the header says, since
/// servers mislabel them; so is one labelled `application/pdf`. Images,
/// audio, video, fonts, archives, office documents and
/// `application/octet-stream` are unsupported files. Everything else is
/// text.
pub fn classify(content_type: Option<&str>, body: &[u8]) -> BodyKind {
    let media = content_type
        .and_then(|value| value.split(';').next())
        .map(|media| media.trim().to_ascii_lowercase())
        .unwrap_or_default();
    if body.starts_with(b"%PDF-") || media == PDF || media == "application/x-pdf" {
        return BodyKind::Pdf;
    }
    if is_other_file(&media) {
        return BodyKind::Unsupported(media);
    }
    BodyKind::Text
}

fn is_other_file(media: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "image/",
        "audio/",
        "video/",
        "font/",
        "model/",
        "application/vnd.ms-",
        "application/vnd.openxmlformats-officedocument.",
        "application/vnd.oasis.opendocument.",
    ];
    const TYPES: &[&str] = &[
        "application/octet-stream",
        "application/zip",
        "application/gzip",
        "application/x-gzip",
        "application/x-tar",
        "application/x-bzip2",
        "application/x-xz",
        "application/zstd",
        "application/x-7z-compressed",
        "application/x-rar-compressed",
        "application/vnd.rar",
        "application/java-archive",
        "application/msword",
        "application/rtf",
        "application/wasm",
    ];
    TYPES.contains(&media)
        || PREFIXES.iter().any(|prefix| media.starts_with(prefix))
        || media.ends_with("+zip")
}

/// The directory a session's delivered files live in: beside its log,
/// `<home>/sessions/<session>.files/`. The session scanners read `.ndjson`
/// files only, so the directory is not mistaken for a log.
pub fn directory_for(session_log: &Path) -> PathBuf {
    session_log.with_extension("files")
}

/// The bare hex SHA-256 of `bytes`, which names the file.
pub fn sha256_hex(bytes: &[u8]) -> String {
    sha256_digest(bytes)
        .strip_prefix("sha256:")
        .map(str::to_owned)
        .unwrap_or_default()
}

/// Where [`save`] puts bytes whose bare hex SHA-256 is `hex`.
pub fn saved_at(directory: &Path, hex: &str) -> PathBuf {
    directory.join(format!("{hex}.pdf"))
}

/// Save `bytes` as `<directory>/<sha256>.pdf`, the directory owner-only
/// (0700) and the file owner-only (0600) whatever the umask, and return the
/// file's path. A file already there whose bytes hash to its name is
/// reused, so a second fetch of the same bytes writes nothing; one whose
/// bytes do not is replaced.
pub fn save(directory: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    private_directory(directory)?;
    let path = saved_at(directory, &sha256_hex(bytes));
    if let Ok(metadata) = std::fs::symlink_metadata(&path)
        && metadata.file_type().is_file()
        && std::fs::read(&path).is_ok_and(|held| held == bytes)
    {
        owner_only(&path, 0o600)?;
        return Ok(path);
    }
    crate::declaration::replace_private(&path, bytes)?;
    Ok(path)
}

/// Create `directory` owner-only, or narrow one that is there to owner-only.
/// A symbolic link there is refused: the files would land wherever it
/// points.
fn private_directory(directory: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.file_type().is_dir() => owner_only(directory, 0o700),
        Ok(_) => Err(format!(
            "{} is there and is not a directory",
            directory.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt as _;
                builder.mode(0o700);
            }
            builder
                .create(directory)
                .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
            // The mode asked at creation is masked by the umask, which can
            // only narrow it; set it again so the result does not depend on
            // one.
            owner_only(directory, 0o700)
        }
        Err(error) => Err(format!("cannot read {}: {error}", directory.display())),
    }
}

#[cfg(unix)]
fn owner_only(path: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|error| format!("cannot set the mode of {}: {error}", path.display()))
}

#[cfg(not(unix))]
fn owner_only(_path: &Path, _mode: u32) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pdf_is_known_by_its_type_or_its_first_bytes() {
        assert_eq!(classify(Some("application/pdf"), b"x"), BodyKind::Pdf);
        assert_eq!(
            classify(Some("Application/PDF; name=a.pdf"), b"x"),
            BodyKind::Pdf
        );
        assert_eq!(
            classify(Some("text/html; charset=utf-8"), b"%PDF-1.7\n"),
            BodyKind::Pdf
        );
        assert_eq!(classify(None, b"%PDF-1.4"), BodyKind::Pdf);
        // `%PDF` alone, without the hyphen, is not the signature.
        assert_eq!(classify(Some("text/plain"), b"%PDF is a"), BodyKind::Text);
    }

    #[test]
    fn other_files_are_named_and_every_other_type_stays_text() {
        for media in [
            "image/png",
            "audio/mpeg",
            "video/mp4",
            "font/woff2",
            "application/octet-stream",
            "application/zip",
            "application/epub+zip",
            "application/msword",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "application/vnd.ms-excel",
            "application/vnd.oasis.opendocument.text",
        ] {
            assert_eq!(
                classify(Some(media), b"\x89PNG"),
                BodyKind::Unsupported(media.to_owned()),
                "{media}"
            );
        }
        for media in [
            None,
            Some("text/html"),
            Some("text/plain; charset=utf-8"),
            Some("application/json"),
            Some("application/xml"),
            Some("application/rss+xml"),
            Some("application/javascript"),
            Some("application/x-unlisted"),
        ] {
            assert_eq!(classify(media, b"<p>text</p>"), BodyKind::Text, "{media:?}");
        }
    }

    #[test]
    fn a_saved_file_is_named_by_its_hash_and_a_second_save_reuses_it() {
        let home = tempfile::tempdir().unwrap();
        let directory = directory_for(&home.path().join("sessions/s.ndjson"));
        std::fs::create_dir_all(home.path().join("sessions")).unwrap();
        let bytes = b"%PDF-1.7\nbody\n%%EOF\n";
        let saved = save(&directory, bytes).unwrap();
        assert_eq!(saved, directory.join(format!("{}.pdf", sha256_hex(bytes))));
        assert_eq!(std::fs::read(&saved).unwrap(), bytes);
        let modified = std::fs::metadata(&saved).unwrap().modified().unwrap();
        assert_eq!(save(&directory, bytes).unwrap(), saved);
        assert_eq!(
            std::fs::metadata(&saved).unwrap().modified().unwrap(),
            modified,
            "the same bytes are not written twice"
        );
        // A file under the name whose bytes do not match it is replaced.
        std::fs::write(&saved, b"tampered").unwrap();
        assert_eq!(save(&directory, bytes).unwrap(), saved);
        assert_eq!(std::fs::read(&saved).unwrap(), bytes);
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_in_place_of_the_directory_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let directory = home.path().join("s.files");
        std::os::unix::fs::symlink(elsewhere.path(), &directory).unwrap();
        let refused = save(&directory, b"%PDF-1.7").unwrap_err();
        assert!(refused.contains("is not a directory"), "{refused}");
        assert_eq!(std::fs::read_dir(elsewhere.path()).unwrap().count(), 0);
    }

    /// The directory is 0700 and the file 0600 under umask 022, in a child
    /// process whose umask is set between fork and exec
    /// ([`crate::test_umask::under_umask_022`]).
    #[cfg(unix)]
    #[test]
    fn the_directory_and_the_file_are_owner_only_under_umask_022() {
        crate::test_umask::under_umask_022(
            "fetched_file::tests::the_directory_and_the_file_are_owner_only_under_umask_022",
            || {
                use std::os::unix::fs::PermissionsExt as _;
                let home = tempfile::tempdir().unwrap();
                let directory = home.path().join("s.files");
                let saved = save(&directory, b"%PDF-1.7\n").unwrap();
                let mode =
                    |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode(&directory), 0o700);
                assert_eq!(mode(&saved), 0o600);
                // A directory left wider by another writer is narrowed, and
                // a reused file is kept owner-only.
                std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755))
                    .unwrap();
                std::fs::set_permissions(&saved, std::fs::Permissions::from_mode(0o644)).unwrap();
                assert_eq!(save(&directory, b"%PDF-1.7\n").unwrap(), saved);
                assert_eq!(mode(&directory), 0o700);
                assert_eq!(mode(&saved), 0o600);
            },
        );
    }
}
