---
title: Releasing
domain: edge
audience: contributor
---

# Releasing

How a release of `commonmeasure` is cut and checked, and how its notes are
written. What a release holds and how an operator installs it are in
`docs/GETTING-STARTED.md` §1.

## One version

The version is `version` under `[workspace.package]` in `Cargo.toml`. Every
crate takes it from there, and the binary reports it:

```sh
commonmeasure --version
```

`plugin/.claude-plugin/plugin.json` and `browser/manifest.json` carry the
same string, because Claude Code and the browser read those manifests and the
operator reads the binary;
`crates/commonmeasure-cli/tests/version_identity.rs` fails when any of them
differ, and the release workflow's version job runs it before anything is
built.
`plugin/package.sh` names the archive from the version the bundled binary
reports and refuses a manifest that disagrees. A release tag is the version
with `v` in front: `v0.4.0` for version `0.4.0`.

## Cutting a release

Set the version in `Cargo.toml`, `plugin/.claude-plugin/plugin.json` and
`browser/manifest.json`, update the workspace packages in `Cargo.lock`,
refresh the Public Suffix List the binary carries with
`cargo update -p psl` (the crate publishes each day the list changes; a
release knows only the suffixes listed on the day its crate was published,
and may ask a suffix added later as though it were a registrable domain),
write the version's `CHANGELOG.md` section (§Release notes) and replace
`(unreleased)` in its heading with the release date, run the gates and
commit. The maintainers' publishing process creates one public commit
carrying that tree on `main` of `github.com/commonmeasure/commonmeasure`,
creates the release tag on that commit and pushes them together. The tag
push starts the release workflow.

The release workflow (`.github/workflows/release.yml`) runs four jobs:

1. **version** builds the binary, fails when the tag is not `v` followed by
   the version the binary reports, runs the identity test (the plugin and
   browser manifests against the workspace version), and fails when
   `CHANGELOG.md` has no dated, non-empty section for the version
   (`.github/scripts/release-notes.sh`). Nothing is built for release until
   the tag, the manifests, the binary and the notes agree.
2. **build** produces one binary per platform the release supports: Linux
   x64 and arm64 (static, musl) and Windows x64, cross-compiled with zig as
   `plugin/build.sh` does, and macOS arm64 and x64, built on Apple silicon.
   The five-entry matrix gives each binary its own runner: three Linux
   runners and two macOS runners. Where the runner can execute what it
   built, the binary must report the tagged version.
3. **publish** packages the plugin archive from those binaries with
   `plugin/package.sh`, writes `SHA256SUMS` over every asset and creates the
   release under the tag, titled `Common Measure <version>`, with the
   version's `CHANGELOG.md` section as its notes.
4. **verify** runs the stricter workflow form of §The clean-container check
   on the published release.

The asset names are those in `docs/GETTING-STARTED.md` §1; the binaries are
named as the plugin's launcher names them. The release is public, and
reading it needs no account, token or GitHub client.

## Release notes

Each release's `CHANGELOG.md` section is its GitHub release notes, and its
readers are operators and integrators, not the team.

- Put breaking changes and required actions first, in a short paragraph
  under the heading: what stops working and what the reader must do.
- Then group the changes under `### Added`, `### Changed`, `### Removed`,
  `### Fixed` and `### Security`, in that order, leaving out empty groups.
- One line per change, saying what a user can now do or must now do, with a
  link to the page that documents it. Use an absolute URL, to the page on
  `commonmeasure.ai/docs` or to the file on GitHub, because the notes are
  read on GitHub.
- No commit hashes, branch or lane names, review or finding ids, or roadmap
  IDs. A security fix names the problem it fixes and the versions affected.
- The section heading is `## <version> (<day> <month> <year>)`, or
  `## <version> (unreleased)` until the release is cut. The release
  workflow reads the section by that heading.
- Earlier sections stay as they were published.

For example, part of 0.4.0 written this way:

```markdown
## 0.4.0 (23 September 2026)

`robots.txt` now binds in every policy mode. If you ran `observe` or
`prefer` and relied on the edge fetching a page that `robots.txt`
disallows, that fetch is now refused.

### Changed

- A `Disallow` for `CommonMeasureBot`, or for `*` where no group names it,
  refuses the fetch in every policy mode
  ([how-to](https://github.com/commonmeasure/commonmeasure/blob/main/docs/integrate/bot.md)).

### Fixed

- A `robots.txt` larger than 512 KiB is read up to its last complete line
  within that bound; earlier releases ignored it and fetched with no rules.
```

## The clean-container check

The acceptance check for a release. The container is Debian with `curl`,
certificates and `git` added and no compiler or Rust toolchain; no token
is passed in; the installer is the one downloaded from the release.

```sh
VERSION=0.4.0   # the release to check
curl -fsSL -o /tmp/commonmeasure-release/install.sh --create-dirs "https://github.com/commonmeasure/commonmeasure/releases/download/v$VERSION/install.sh"
podman run --rm -e TAG="v$VERSION" -e VERSION="$VERSION" -e REPOSITORY=commonmeasure/commonmeasure \
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

A passing check shows, in order: no toolchain; `installed
/root/.local/bin/commonmeasure: commonmeasure <version>, checksum verified`,
then the note that the directory is not on `PATH`; the archive unpacked with
its checksum verified; `commonmeasure <version>` from the installed binary;
the archive's marketplace added and the plugin installed, listed at the
version and enabled; `commonmeasure <version>` from the launcher inside
Claude Code's plugin cache, which runs the release's bundled Linux binary;
the plugin uninstalled and that marketplace removed; the repository added as
a marketplace and the plugin installed from it, listed at the version and
enabled; and `commonmeasure <version>` from that launcher, which bundles no
binary and finds the installed one.

The `verify` job runs a stricter form on every tag in a Linux x64 container
on GitHub against the release just published. It fails if `cargo`, `rustc`
or `cc` is present, checks that the installed binary reports exactly
`commonmeasure <version>`, and checks both plugin listings for the release
version. The manual commands above print these results for inspection; their
toolchain probe also names `gcc`.

What this check covers: the installer, the archive and both marketplace
routes on Linux, x64 in the workflow and arm64 where the check is run on an
Apple silicon machine. The macOS arm64 binary is built and run on the
workflow's macOS runner; the macOS x64, Linux arm64 (in the workflow) and
Windows binaries are built and checksummed by the same run. Where the
installer has been run by hand is in `docs/GETTING-STARTED.md` §1.
`crates/commonmeasure-cli/tests/installer.rs` drives both installer forms
and every refusal against a loopback origin standing where the release
stands.
