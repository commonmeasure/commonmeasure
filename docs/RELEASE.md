---
title: Releases
---

# Releases

How a release of `commonmeasure` is cut, what it holds, and how a person
installs it with neither this repository nor a Rust toolchain. The harness
plugin and the host are defined in [`docs/GLOSSARY.md`](GLOSSARY.md).

## One version

The version is `version` under `[workspace.package]` in `Cargo.toml`. Every
crate takes it from there, and the binary reports it:

```sh
commonmeasure --version
```

`plugin/.claude-plugin/plugin.json` carries the same string, because Claude
Code reads that manifest and the operator reads the binary;
`crates/commonmeasure-cli/tests/version_identity.rs` fails when they differ.
`browser/manifest.json` carries the same version; the harness registration
test checks it against the binary.
`plugin/package.sh` names the archive from the version the bundled binary
reports and refuses a manifest that disagrees. A release tag is the version
with `v` in front: `v0.3.1` for version `0.3.1`.

## Cutting a release

Set the version in `Cargo.toml`, `plugin/.claude-plugin/plugin.json` and
`browser/manifest.json`, update the workspace packages in `Cargo.lock`,
replace `(unreleased)` in the version's `CHANGELOG.md` heading with the
release date, run the gates, commit, then tag and push the tag to the
product repository:

```sh
git tag -a v0.3.1 -m "commonmeasure 0.3.1"
git push origin v0.3.1
```

The release workflow (`.github/workflows/release.yml`) runs four jobs:

1. **version** builds the binary, fails when the tag is not `v` followed by
   the version the binary reports, runs the identity test, and fails when
   `CHANGELOG.md` has no dated section for the version
   (`.github/scripts/release-notes.sh`). Nothing is built for release until
   the tag, the manifests, the binary and the notes agree.
2. **build** produces one binary per platform the release supports: Linux
   x64 and arm64 (static, musl) and Windows x64, cross-compiled with zig on
   one Linux runner as `plugin/build.sh` does, and macOS arm64 and x64,
   built on one Apple Silicon runner. Where the runner can execute what it
   built, the built binary must report the tagged version.
3. **publish** packages the plugin archive from those binaries with
   `plugin/package.sh`, writes `SHA256SUMS` over every asset and creates the
   release under the tag, titled with the version, with the version's
   `CHANGELOG.md` section as its notes.
4. **verify** repeats §The clean-container check on the published release.

A release holds:

- `commonmeasure-linux-x64`, `commonmeasure-linux-arm64`,
  `commonmeasure-darwin-arm64`, `commonmeasure-darwin-x64` and
  `commonmeasure-win-x64.exe`: the binaries, named as the plugin's launcher
  names them.
- `commonmeasure-plugin-<version>.tar.gz`: the plugin archive, its own
  Claude Code marketplace, bundling those five binaries.
- `install.sh`: the installer.
- `SHA256SUMS`: the SHA-256 of every other asset.

The release is public. Reading it needs no account, token or GitHub client.

## Installing from a release

`install.sh` needs `curl` and `sha256sum` or `shasum`, and no account. One
line downloads and runs it against the latest release:

```sh
curl -fsSL https://github.com/commonmeasure/commonmeasure/releases/latest/download/install.sh | sh
```

To pin a release, download the installer into a directory of its own (a
checkout has an `install.sh` of its own at the root) and name the tag:
`curl -fsSL -o /tmp/commonmeasure-release/install.sh --create-dirs
https://github.com/commonmeasure/commonmeasure/releases/download/v0.3.1/install.sh`,
then `sh /tmp/commonmeasure-release/install.sh --tag v0.3.1`. The pinned
form is what §The clean-container check runs, and each release run's
`verify` job runs it on GitHub against the release that run published. The
one-line form was run on macOS from an empty home directory, and its output
is quoted in [`docs/GETTING-STARTED.md`](GETTING-STARTED.md)
§1. `crates/commonmeasure-cli/tests/installer.rs` drives both forms and
every refusal against a loopback origin standing where the release stands.

It detects the platform, downloads `SHA256SUMS` and that platform's binary
from the release's download address, verifies the checksum, moves the
binary to `~/.local/bin/commonmeasure` (`--dir` chooses another directory)
and checks that the installed binary reports the release's version; a
binary that fails either check is removed. Without `--tag` it installs the
latest release, found from the redirect the release page answers with. With
the binary in place, `commonmeasure install claude` registers it with
Claude Code by absolute path ([`docs/GETTING-STARTED.md`](GETTING-STARTED.md) §1).
`--plugin DIR` also downloads and verifies the plugin archive, unpacks it
into `DIR` and prints the two commands that install it into Claude Code;
the archive is the one copy of the plugin that bundles binaries.

The repository is also a Claude Code marketplace. With the binary installed,
this registers the plugin from the repository, and the plugin's launcher
finds the binary on `PATH` or in `~/.local/bin`:

```sh
claude plugin marketplace add commonmeasure/commonmeasure
claude plugin install commonmeasure@commonmeasure
```

The installer names what it cannot do and stops ([`docs/FAIL-POLICY.md`](FAIL-POLICY.md) §5):
no `curl`, no checksum tool, a release that does not exist, or a platform
the release has no binary for, in which case it lists the binaries the
release holds. It does not edit shell profiles; when the install directory
is not on `PATH` it prints the line to add. `COMMONMEASURE_RELEASE_URL`
points it at another origin with the same asset names and address shape,
for a mirror or a test. `crates/commonmeasure-cli/tests/installer.rs`
drives each refusal against a loopback origin standing where the release
stands.

## The clean-container check

The acceptance check for a release. The container is Debian with `curl`,
certificates and `git` added and no compiler or Rust toolchain; no token
is passed in; the installer is the one downloaded from the release.

```sh
curl -fsSL -o /tmp/commonmeasure-release/install.sh --create-dirs https://github.com/commonmeasure/commonmeasure/releases/download/v0.3.1/install.sh
podman run --rm -e TAG=v0.3.1 -e VERSION=0.3.1 -e REPOSITORY=commonmeasure/commonmeasure \
  -v /tmp/commonmeasure-release/install.sh:/install.sh:ro,z \
  docker.io/library/debian:bookworm-slim sh -c '
set -e
apt-get update -qq >/dev/null && apt-get install -y -qq curl ca-certificates git >/dev/null 2>&1
command -v cargo rustc cc gcc || echo "no cargo, rustc, cc or gcc"
sh /install.sh --tag "$TAG" --plugin /opt/commonmeasure
export PATH="$HOME/.local/bin:$PATH"
commonmeasure --version
curl -fsSL https://claude.ai/install.sh | bash >/dev/null 2>&1
cd /opt/commonmeasure/commonmeasure-plugin
claude plugin marketplace add ./
claude plugin install commonmeasure@commonmeasure
claude plugin list
"$HOME/.claude/plugins/cache/commonmeasure/commonmeasure/$VERSION/bin/commonmeasure-launch" --version
claude plugin uninstall commonmeasure@commonmeasure
claude plugin marketplace remove commonmeasure
claude plugin marketplace add "$REPOSITORY"
claude plugin install commonmeasure@commonmeasure
claude plugin list
"$HOME/.claude/plugins/cache/commonmeasure/commonmeasure/$VERSION/bin/commonmeasure-launch" --version
'
```

The output shows, in order: no toolchain; `installed
/root/.local/bin/commonmeasure: commonmeasure 0.3.1, checksum verified`, then
the note that the directory is not on `PATH`; the archive unpacked with its
checksum verified; `commonmeasure 0.3.1` from the installed binary; the
archive's marketplace added and the plugin installed, listed at version
0.3.1 and enabled; `commonmeasure 0.3.1` from the launcher inside Claude
Code's plugin cache, which runs the release's bundled Linux binary; the
plugin uninstalled and that marketplace removed; the repository added as a
marketplace and the plugin installed from it, listed at version 0.3.1 and
enabled; and `commonmeasure 0.3.1` from that launcher, which bundles no
binary and finds the installed one.

The `verify` job of the release workflow runs the same check on every tag,
in a Linux x64 container on GitHub against the release the run has just
published.

What this check covers: the installer, the archive and both marketplace
routes on Linux, x64 in the workflow and arm64 where the check is run on an
Apple Silicon machine. The macOS arm64 binary is built and run on the
workflow's macOS runner; the macOS x64, Linux arm64 (in the workflow) and
Windows binaries are built and checksummed by the same run. The one-line
installer has been run on macOS Apple silicon from an empty home directory
([`docs/GETTING-STARTED.md`](GETTING-STARTED.md) §1); the installer has
not been run on Windows.
