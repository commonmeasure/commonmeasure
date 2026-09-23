//! Exact tracked blob binding from one immutable Git tree.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::Path,
    process::{Command, Stdio},
};

use commonmeasure_types::canonical::{canonical_digest, sha256_digest};
use serde_json::{Value, json};
use unicode_normalization::UnicodeNormalization;

const MAX_ENTRIES: usize = 10_000;
const MAX_TREE_BYTES: usize = 8 * 1024 * 1024;
const MAX_BLOB_BYTES: usize = 64 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 256 * 1024 * 1024;

/// Bind every regular tracked blob in `revision`'s fixed tree, except the
/// profile's provenance paths. Working, untracked and ignored files are outside
/// this scope. Unsupported paths, entry kinds and unresolved LFS pointers fail.
pub fn capture(repo: &Path, revision: &str) -> Result<Value, String> {
    // The store exclusion check is relative to this path. Accepting a nested
    // directory while hashing the full tree would give it a different root.
    if git(repo, &["rev-parse", "--show-prefix"], 4096)? != b"\n" {
        return Err("Git artifact path must name the repository root".into());
    }
    if revision.is_empty()
        || revision.len() > 4096
        || revision.starts_with('-')
        || revision.chars().any(char::is_control)
    {
        return Err("invalid Git revision".into());
    }
    let format = git(repo, &["rev-parse", "--show-object-format=storage"], 128)?;
    let format = text_line(&format)?;
    let oid_length = match format {
        "sha1" => 40,
        "sha256" => 64,
        _ => return Err("unsupported Git object format".into()),
    };
    // Never resolve the caller's revision again after this operation.
    let tree = git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{revision}^{{tree}}"),
        ],
        128,
    )?;
    let tree = text_line(&tree)?;
    validate_oid(tree, oid_length)?;
    let listing = git(
        repo,
        &["ls-tree", "-r", "-t", "-z", "-l", "--full-tree", tree],
        MAX_TREE_BYTES,
    )?;
    let entries = inventory(repo, &listing, oid_length)?;
    let inventory = json!({
        "scope": "repo-inventory/1",
        "excluded_prefixes": [".commonmeasure/"],
        "excluded_paths": ["COMMONMEASURE.md"],
        "entries": entries,
    });
    Ok(json!({
        "scope": "repo-inventory/1",
        "algorithm": "sha256",
        "digest": canonical_digest(&inventory),
        "source": { "object_format": format, "tree": tree },
        "inventory": inventory,
    }))
}

fn inventory(repo: &Path, listing: &[u8], oid_length: usize) -> Result<Vec<Value>, String> {
    if !listing.is_empty() && !listing.ends_with(&[0]) {
        return Err("unterminated Git tree listing".into());
    }
    let mut names = BTreeMap::new();
    let mut trees = BTreeSet::new();
    let mut parents = BTreeSet::new();
    let mut blobs = BTreeMap::new();
    let mut total = 0usize;
    for (count, record) in listing.split_inclusive(|byte| *byte == 0).enumerate() {
        if count >= MAX_ENTRIES {
            return Err("Git tree exceeds 10,000 entries".into());
        }
        let record = record
            .strip_suffix(&[0])
            .ok_or("unterminated Git tree entry")?;
        let record = std::str::from_utf8(record).map_err(|_| "non-UTF-8 Git path")?;
        let (metadata, path) = record.split_once('\t').ok_or("invalid Git tree entry")?;
        validate_path(path)?;
        let folded = folded_path(path);
        if names.insert(folded, path).is_some() {
            return Err(format!("duplicate or case-colliding Git path: {path}"));
        }
        let fields: Vec<_> = metadata.split_ascii_whitespace().collect();
        let [mode, kind, oid, size] = fields.as_slice() else {
            return Err("invalid Git tree metadata".into());
        };
        validate_oid(oid, oid_length)?;
        if let Some((parent, _)) = path.rsplit_once('/') {
            parents.insert(parent);
        }
        if path.starts_with(".commonmeasure/")
            || (path == ".commonmeasure" && *kind == "tree")
            || (path == "COMMONMEASURE.md" && *kind != "tree")
        {
            continue;
        }
        if *mode == "040000" && *kind == "tree" && *size == "-" {
            trees.insert(path);
            continue;
        }
        if !matches!(*mode, "100644" | "100755") || *kind != "blob" {
            return Err(format!(
                "unsupported Git entry (symlink, submodule or mode): {path}"
            ));
        }
        let size: usize = size.parse().map_err(|_| "invalid Git blob size")?;
        total = total.checked_add(size).ok_or("Git tree size overflow")?;
        if size > MAX_BLOB_BYTES || total > MAX_TOTAL_BYTES {
            return Err("Git content exceeds 64 MiB per blob or 256 MiB total".into());
        }
        blobs.insert(path, (*mode, *oid, size));
    }
    if !trees.is_subset(&parents) {
        return Err("empty Git directories are unsupported by repo-inventory/1".into());
    }
    // BTreeMap preserves the profile's case-sensitive UTF-8 byte path order.
    blobs
        .into_iter()
        .map(|(path, (mode, oid, size))| {
            let bytes = git(repo, &["cat-file", "blob", oid], size)?;
            if bytes.len() != size {
                return Err(format!("Git blob length changed: {path}"));
            }
            if bytes.starts_with(b"version https://git-lfs.github.com/spec/v1") {
                return Err(format!("unresolved Git LFS pointer: {path}"));
            }
            Ok(json!({
                "path": path, "mode": mode, "byte_length": size,
                "digest": sha256_digest(&bytes),
            }))
        })
        .collect()
}

pub(super) fn folded_path(path: &str) -> String {
    path.nfc()
        .flat_map(char::to_lowercase)
        .flat_map(char::to_uppercase)
        .flat_map(char::to_lowercase)
        .collect()
}

pub(super) fn validate_path(path: &str) -> Result<(), String> {
    if path.len() > 4096
        || path
            .chars()
            .any(|c| c.is_control() || c == '\\' || c == ':')
    {
        return Err(format!("unsafe Git path: {path:?}"));
    }
    for part in path.split('/') {
        let folded = part.to_ascii_lowercase();
        let stem = folded.split('.').next().unwrap_or_default();
        let device = matches!(stem, "con" | "prn" | "aux" | "nul")
            || (stem.len() == 4
                && (stem.starts_with("com") || stem.starts_with("lpt"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'));
        if part.is_empty()
            || part.len() > 255
            || matches!(part, "." | "..")
            || folded == ".git"
            || part.ends_with(['.', ' '])
            || device
        {
            return Err(format!("unsafe Git path: {path:?}"));
        }
    }
    Ok(())
}

fn validate_oid(oid: &str, length: usize) -> Result<(), String> {
    if oid.len() != length
        || !oid
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err("invalid Git object ID".into());
    }
    Ok(())
}

fn text_line(bytes: &[u8]) -> Result<&str, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "invalid Git output")?;
    let text = text.strip_suffix('\n').ok_or("invalid Git output")?;
    if text.is_empty() || text.chars().any(char::is_control) {
        return Err("ambiguous Git output".into());
    }
    Ok(text)
}

fn git(repo: &Path, args: &[&str], limit: usize) -> Result<Vec<u8>, String> {
    let mut command = Command::new("git");
    // Inherited Git variables must not redirect the repository, object database
    // or config. Plumbing reads raw objects; transports remain forbidden even on
    // Git versions which do not recognise GIT_NO_LAZY_FETCH.
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            command.env_remove(name);
        }
    }
    command
        .current_dir(repo)
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .args([
            "--no-pager",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "protocol.allow=never",
            "-c",
            "core.warnAmbiguousRefs=true",
        ])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| format!("Git unavailable: {e}"))?;
    let stderr = child.stderr.take().ok_or("missing Git stderr")?;
    let errors = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.take(8193).read_to_end(&mut bytes).map(|_| bytes)
    });
    let mut bytes = Vec::new();
    let read = child
        .stdout
        .take()
        .ok_or("missing Git stdout")?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes);
    if read.is_err() || bytes.len() > limit {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|e| format!("Git wait failed: {e}"))?;
    let errors = errors
        .join()
        .map_err(|_| "Git stderr reader failed")?
        .map_err(|e| format!("Git stderr read failed: {e}"))?;
    read.map_err(|e| format!("Git read failed: {e}"))?;
    if bytes.len() > limit {
        return Err("Git output exceeds capture limit".into());
    }
    // Warnings include ambiguous revision names. Refuse them even if Git picked
    // an object and exited successfully.
    if !status.success() || !errors.is_empty() {
        return Err(format!(
            "Git capture failed: {}",
            String::from_utf8_lossy(&errors)
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    struct Repo(tempfile::TempDir);

    impl Repo {
        fn new() -> Self {
            let repo = Self(tempfile::tempdir().unwrap());
            repo.run(&["init", "-q"], b"");
            repo
        }

        fn run(&self, args: &[&str], input: &[u8]) -> String {
            let mut child = Command::new("git")
                .current_dir(self.0.path())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_AUTHOR_NAME", "Fixture")
                .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
                .env("GIT_COMMITTER_NAME", "Fixture")
                .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(input).unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout)
                .unwrap()
                .trim_end()
                .to_owned()
        }

        fn blob(&self, bytes: &[u8]) -> String {
            self.run(&["hash-object", "-w", "--stdin"], bytes)
        }

        fn tree(&self, entries: &[(&str, &str, &[u8])]) -> String {
            let mut input = Vec::new();
            for (mode, oid, name) in entries {
                let kind = match *mode {
                    "040000" => "tree",
                    "160000" => "commit",
                    _ => "blob",
                };
                input.extend_from_slice(format!("{mode} {kind} {oid}\t").as_bytes());
                input.extend_from_slice(name);
                input.push(0);
            }
            self.run(&["mktree", "-z"], &input)
        }

        fn capture(&self, tree: &str) -> Result<Value, String> {
            capture(self.0.path(), tree)
        }
    }

    #[test]
    fn binds_exact_bytes_modes_and_complete_inventory() {
        let repo = Repo::new();
        let original = repo.blob(b"code\0\xff\r\n");
        let changed = repo.blob(b"changed\n");
        let tree = repo.tree(&[("100644", &original, b"main.rs")]);
        let first = repo.capture(&tree).unwrap();
        assert_eq!(
            first["inventory"]["entries"][0]["digest"],
            sha256_digest(b"code\0\xff\r\n")
        );
        assert_eq!(first["digest"], canonical_digest(&first["inventory"]));
        for entries in [
            vec![("100644", changed.as_str(), b"main.rs".as_slice())],
            vec![("100755", original.as_str(), b"main.rs".as_slice())],
            vec![
                ("100644", original.as_str(), b"main.rs".as_slice()),
                ("100644", original.as_str(), b"new.rs".as_slice()),
            ],
            vec![],
        ] {
            assert_ne!(
                first["digest"],
                repo.capture(&repo.tree(&entries)).unwrap()["digest"]
            );
        }
        std::fs::write(repo.0.path().join("main.rs"), b"working-tree drift").unwrap();
        std::fs::write(repo.0.path().join("untracked"), b"outside scope").unwrap();
        assert_eq!(repo.capture(&tree).unwrap(), first);
    }

    #[test]
    fn metadata_only_commits_change_source_but_keep_payload() {
        let repo = Repo::new();
        let code = repo.blob(b"payload");
        let old = repo.blob(b"old record");
        let new = repo.blob(b"new record");
        let mut captures = Vec::new();
        for metadata in [old, new] {
            let records = repo.tree(&[("100644", &metadata, b"record.json")]);
            let tree = repo.tree(&[
                ("040000", &records, b".commonmeasure"),
                ("100644", &metadata, b"COMMONMEASURE.md"),
                ("100644", &code, b"main.rs"),
            ]);
            let commit = repo.run(&["commit-tree", &tree, "-m", "fixture"], b"");
            captures.push(repo.capture(&commit).unwrap());
        }
        assert_eq!(captures[0]["digest"], captures[1]["digest"]);
        assert_ne!(captures[0]["source"]["tree"], captures[1]["source"]["tree"]);
        assert_eq!(
            captures[0]["inventory"]["entries"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let nested = repo.tree(&[("100644", &code, b"COMMONMEASURE.md")]);
        let tree = repo.tree(&[
            ("040000", &nested, b"docs"),
            ("100644", &code, b".commonmeasure-other"),
        ]);
        assert_eq!(
            repo.capture(&tree).unwrap()["inventory"]["entries"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn refuses_symlinks_submodules_and_lfs_pointers() {
        let repo = Repo::new();
        let blob = repo.blob(b"../outside");
        let link = repo.tree(&[("120000", &blob, b"link")]);
        assert!(
            repo.capture(&link)
                .unwrap_err()
                .contains("unsupported Git entry")
        );
        let empty = repo.tree(&[]);
        let commit = repo.run(&["commit-tree", &empty, "-m", "fixture"], b"");
        let module = repo.tree(&[("160000", &commit, b"module")]);
        assert!(
            repo.capture(&module)
                .unwrap_err()
                .contains("unsupported Git entry")
        );
        let lfs =
            repo.blob(b"version https://git-lfs.github.com/spec/v1\noid sha256:abcd\nsize 4\n");
        let tree = repo.tree(&[("100644", &lfs, b"large.bin")]);
        assert!(repo.capture(&tree).unwrap_err().contains("LFS pointer"));
    }

    #[test]
    fn refuses_unsafe_non_utf8_and_colliding_paths() {
        let repo = Repo::new();
        let blob = repo.blob(b"payload");
        for name in [
            b"back\\slash".as_slice(),
            b"colon:name",
            b"line\nbreak",
            b"trailing.",
            b"CON",
            b"bad\xff",
        ] {
            let tree = repo.tree(&[("100644", &blob, name)]);
            assert!(repo.capture(&tree).is_err(), "accepted {name:?}");
        }
        for (a, b) in [("Name", "name"), ("Straße", "STRASSE"), ("é", "e\u{301}")] {
            let tree = repo.tree(&[
                ("100644", &blob, a.as_bytes()),
                ("100644", &blob, b.as_bytes()),
            ]);
            assert!(repo.capture(&tree).unwrap_err().contains("colliding"));
        }
        let child = repo.tree(&[("100644", &blob, b"file")]);
        let tree = repo.tree(&[("040000", &child, b"Dir"), ("040000", &child, b"dir")]);
        assert!(repo.capture(&tree).unwrap_err().contains("colliding"));
    }

    #[test]
    fn ignores_replacements_and_never_runs_content_filters() {
        let repo = Repo::new();
        let blob = repo.blob(b"original");
        let replacement = repo.blob(b"replacement");
        let attrs = repo.blob(b"* filter=unexpected\n");
        let tree = repo.tree(&[
            ("100644", &blob, b"main.rs"),
            ("100644", &attrs, b".gitattributes"),
        ]);
        let before = repo.capture(&tree).unwrap();
        repo.run(&["replace", &blob, &replacement], b"");
        repo.run(&["config", "filter.unexpected.smudge", "false"], b"");
        repo.run(&["config", "filter.unexpected.required", "true"], b"");
        assert_eq!(repo.capture(&tree).unwrap(), before);
        let replacement_tree = repo.tree(&[("100644", &replacement, b"other")]);
        repo.run(&["replace", &tree, &replacement_tree], b"");
        assert_eq!(repo.capture(&tree).unwrap(), before);
    }

    #[test]
    fn rejects_revision_injection_ambiguity_and_missing_objects() {
        let repo = Repo::new();
        let tree = repo.tree(&[]);
        let commit = repo.run(&["commit-tree", &tree, "-m", "fixture"], b"");
        repo.run(&["update-ref", "refs/heads/same", &commit], b"");
        repo.run(&["update-ref", "refs/tags/same", &commit], b"");
        assert!(repo.capture("same").is_err());
        for revision in ["", "--help", "--output=/tmp/injected", "HEAD\n", "missing"] {
            assert!(repo.capture(revision).is_err());
        }
    }

    #[test]
    fn sha256_repository_uses_full_tree_locator() {
        let repo = Repo(tempfile::tempdir().unwrap());
        repo.run(&["init", "-q", "--object-format=sha256"], b"");
        let blob = repo.blob(b"payload");
        let tree = repo.tree(&[("100644", &blob, b"file")]);
        let binding = repo.capture(&tree).unwrap();
        assert_eq!(binding["source"]["object_format"], "sha256");
        assert_eq!(binding["source"]["tree"], tree);
        assert_eq!(tree.len(), 64);
    }

    #[test]
    fn missing_promised_blob_cannot_start_a_transport() {
        let repo = Repo::new();
        let blob = repo.blob(b"payload");
        let tree = repo.tree(&[("100644", &blob, b"file")]);
        repo.run(&["config", "remote.origin.promisor", "true"], b"");
        repo.run(
            &["config", "remote.origin.url", "ext::touch transport-ran"],
            b"",
        );
        repo.run(&["config", "protocol.ext.allow", "always"], b"");
        std::fs::remove_file(
            repo.0
                .path()
                .join(".git/objects")
                .join(&blob[..2])
                .join(&blob[2..]),
        )
        .unwrap();
        assert!(repo.capture(&tree).is_err());
        assert!(!repo.0.path().join("transport-ran").exists());
    }

    #[test]
    fn utf8_byte_order_and_original_unicode_are_preserved() {
        let repo = Repo::new();
        let blob = repo.blob(b"payload");
        let names = ["z", "é", "e\u{301}x", "a"];
        let entries: Vec<_> = names
            .iter()
            .map(|name| ("100644", blob.as_str(), name.as_bytes()))
            .collect();
        let binding = repo.capture(&repo.tree(&entries)).unwrap();
        let paths: Vec<_> = binding["inventory"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths, ["a", "e\u{301}x", "z", "é"]);
    }

    #[test]
    fn bounded_output_and_inventory_refuse_excess_and_empty_subtrees() {
        let repo = Repo::new();
        let blob = repo.blob(b"12345");
        assert!(git(repo.0.path(), &["cat-file", "blob", &blob], 4).is_err());
        let empty = repo.tree(&[]);
        let tree = repo.tree(&[("040000", &empty, b"empty")]);
        assert!(
            repo.capture(&tree)
                .unwrap_err()
                .contains("empty Git directories")
        );
        let oversized = format!("100644 blob {blob} {}\tfile\0", MAX_BLOB_BYTES + 1);
        assert!(
            inventory(repo.0.path(), oversized.as_bytes(), 40)
                .unwrap_err()
                .contains("64 MiB")
        );
        let listing: String = (0..=MAX_ENTRIES)
            .map(|i| format!("100644 blob {blob} 5\tfile{i}\0"))
            .collect();
        assert!(
            inventory(repo.0.path(), listing.as_bytes(), 40)
                .unwrap_err()
                .contains("10,000")
        );
        let listing: String = (0..5)
            .map(|i| format!("100644 blob {blob} {MAX_BLOB_BYTES}\tfile{i}\0"))
            .collect();
        assert!(
            inventory(repo.0.path(), listing.as_bytes(), 40)
                .unwrap_err()
                .contains("256 MiB")
        );
    }
}
