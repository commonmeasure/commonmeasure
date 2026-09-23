#!/bin/sh
# Package the browser extension as a zip archive holding exactly the files
# Chrome loads: the manifest, the scripts and the popup. Tests, this script and
# the README stay out. Chrome loads an unpacked extension from a directory, so
# the recipient unzips the archive and chooses the directory in
# chrome://extensions with Developer mode on.
#
# The archive's version is the manifest's, which must equal the workspace
# version in Cargo.toml, because the extension and the binary it talks to are
# one release.
#
# Output: dist/commonmeasure-browser-<version>.zip. `dist/` is a build output
# and is gitignored.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo=$(CDPATH= cd -- "$here/.." && pwd)

version=$(sed -n 's/^  "version": "\(.*\)",$/\1/p' "$here/manifest.json")
workspace=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$repo/Cargo.toml" | head -n 1)
[ -n "$version" ] || { echo "cannot read version from browser/manifest.json" >&2; exit 1; }
if [ "$version" != "$workspace" ]; then
  echo "version mismatch: browser/manifest.json says $version, Cargo.toml says $workspace" >&2
  exit 1
fi

files="manifest.json background.js parse.js message.js inject.js content.js google.js bing.js popup.html popup.js"
archive="$repo/dist/commonmeasure-browser-$version.zip"
mkdir -p "$repo/dist"
rm -f "$archive"
cd "$here"
# -X leaves out macOS extended attributes.
zip -X "$archive" $files
echo "$archive"
