#!/bin/sh
# Package the plugin as a standalone, installable archive: a directory that is
# its own Claude Code marketplace, so the recipient runs
#
#   tar xzf commonmeasure-plugin-<version>.tar.gz
#   claude plugin marketplace add ./commonmeasure-plugin
#   claude plugin install commonmeasure@commonmeasure
#
# with no repository access and no Rust toolchain. The archive carries whatever
# prebuilt binaries `plugin/bin/` holds when this runs; build them first with
# plugin/build.sh (macOS builds every platform via zig cross-compilation; Linux
# builds its own platform statically through zig when cargo-zigbuild is
# present and natively otherwise). Packaging refuses to produce an archive
# with no binaries at all — an empty package would install a plugin whose every
# surface says "no prebuilt binary".
#
# The archive's version is the one the bundled binary for this platform
# reports, which is the workspace version it was compiled with. The plugin
# manifest must carry the same string, because Claude Code reads the manifest
# and the operator reads the binary, and the two must name one release.
#
# Output: dist/commonmeasure-plugin-<version>.tar.gz and the unpacked staging
# directory beside it. `dist/` is a build output and is gitignored.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo=$(CDPATH= cd -- "$here/.." && pwd)

binaries=$(find "$here/bin" -name 'commonmeasure-*' ! -name 'commonmeasure-launch' | sort)
if [ -z "$binaries" ]; then
  echo "plugin/bin holds no prebuilt binaries; run plugin/build.sh first" >&2
  exit 1
fi

reported=$("$here/bin/commonmeasure-launch" --version) || {
  echo "cannot read the version: plugin/bin holds no binary this platform can run, or the one it holds predates version reporting; run plugin/build.sh" >&2
  exit 1
}
version=${reported#commonmeasure }
manifest_version=$(sed -n 's/^  "version": "\(.*\)",$/\1/p' "$here/.claude-plugin/plugin.json")
[ -n "$manifest_version" ] || { echo "cannot read version from plugin.json" >&2; exit 1; }
if [ "$manifest_version" != "$version" ]; then
  echo "version mismatch: the bundled binary reports $version, plugin.json says $manifest_version" >&2
  exit 1
fi

dist="$repo/dist"
stage="$dist/commonmeasure-plugin"
rm -rf "$stage"
mkdir -p "$stage/.claude-plugin"

# The staged copy of the plugin: manifests, hooks, launcher and binaries.
# build.sh and this script stay home; they are how the package is made, not
# part of what it installs.
mkdir -p "$stage/plugin"
cp -R "$here/.claude-plugin" "$here/.mcp.json" "$here/hooks" "$here/bin" \
      "$here/README.md" "$stage/plugin/"

# A marketplace manifest written for the shipped copy, not the working
# checkout the in-repo one describes.
cat > "$stage/.claude-plugin/marketplace.json" <<EOF
{
  "\$schema": "https://anthropic.com/claude-code/marketplace.schema.json",
  "name": "commonmeasure",
  "description": "The Common Measure plugin for Claude Code, packaged for installation from this directory.",
  "owner": {
    "name": "Common Measure"
  },
  "plugins": [
    {
      "name": "commonmeasure",
      "description": "Input control and audit for AI agents. Records the external sources your agent retrieves and grounds on, and offers mediated fetch and search that operator policy can refuse before the crossing.",
      "author": {
        "name": "Common Measure"
      },
      "category": "productivity",
      "source": "./plugin"
    }
  ]
}
EOF

cat > "$stage/INSTALL.md" <<'EOF'
# Installing the Common Measure plugin

From the directory this file is in:

```sh
claude plugin marketplace add ./
claude plugin install commonmeasure@commonmeasure
```

Then start (or restart) a Claude Code session and do some ordinary work that
reads the web. Check what was recorded:

```sh
~/.claude/plugins/cache/commonmeasure/commonmeasure/*/bin/commonmeasure-launch session
~/.claude/plugins/cache/commonmeasure/commonmeasure/*/bin/commonmeasure-launch serve
```

`session` prints what the current session retrieved and grounded on; `serve`
starts the operator console on loopback. The record lives in
`~/.commonmeasure/`, on your machine.

**Nothing leaves your machine.** There is no default telemetry receiver, no
account, and no network call the plugin makes on its own behalf. Localhost,
private-network and `file://` traffic is never recorded at all, except for
prefixes you name yourself in `record_internal_prefixes` in
`~/.commonmeasure/policy.json` — the one way to put an internal corpus on the
record, consent in writing, off unless you write it. Those crossings are
marked internal and are never projected outward. Egress exists only as
`commonmeasure relay`, which refuses to run until you explicitly configure a
receiver.

EOF

# The platform paragraph names what this archive actually bundles: a fixed
# example list would advertise a binary exactly when it is absent (a
# Linux-built archive naming darwin).
bundled=""
for binary in $binaries; do
  name=$(basename "$binary")
  bundled="${bundled:+$bundled, }\`$name\`"
done
cat >> "$stage/INSTALL.md" <<EOF
**Supported platforms** are exactly the binaries bundled in \`plugin/bin/\`:
$bundled. On Windows, Claude Code runs hooks through Git Bash, which ships
with Git for Windows; WSL uses the Linux binaries. If your platform is not
among those bundled the plugin stays harmless — hooks note the missing binary
and let the agent continue — and any Rust toolchain can produce the binary:
\`cargo build --release -p commonmeasure-cli\` in the source repository.

The Linux binaries of a release are statically linked and run on any Linux.
A Linux binary built by \`plugin/build.sh\` on a machine without
\`cargo-zigbuild\` is linked against that machine's glibc and runs only where
that glibc version or newer is present.

EOF

cat >> "$stage/INSTALL.md" <<'EOF'
## Turning it off

- Pause capture for a machine without uninstalling:
  `claude plugin disable commonmeasure@commonmeasure` (re-enable with
  `claude plugin enable`).
- Remove it: `claude plugin uninstall commonmeasure@commonmeasure`. The plugin
  declares its hooks and MCP server in its own manifest, so uninstalling
  removes every surface; nothing was written into your global settings.
- The evidence is yours: uninstalling deliberately leaves `~/.commonmeasure/`
  in place. Delete that directory to delete the record.
EOF

archive="$dist/commonmeasure-plugin-$version.tar.gz"
# The archive carries no extended attributes and no macOS resource forks:
# a Mac's tar otherwise writes its provenance attribute as a pax header that
# tar on every other platform reports as unknown.
( cd "$dist" && COPYFILE_DISABLE=1 tar --no-xattrs -czf "$(basename "$archive")" commonmeasure-plugin )

echo "packaged $archive"
echo "bundled binaries:"
for binary in $binaries; do basename "$binary"; done
