#!/bin/sh
# Install the prebuilt `commonmeasure` binary for this platform from a public
# release of the product repository, github.com/commonmeasure/commonmeasure:
#
#   sh install.sh [--tag v0.3.1] [--dir DIR] [--plugin DIR]
#
# A release holds one binary per supported platform, the plugin archive and
# SHA256SUMS over every asset. The installer picks the binary for this
# platform, downloads it with the checksum list, verifies the checksum before
# the binary is placed on PATH, and checks that the installed binary reports
# the release's version. Without --tag it installs the latest release.
# --dir chooses where the binary goes; the default is ~/.local/bin. --plugin
# also downloads and verifies the plugin archive, unpacks it into DIR and
# prints the commands that install it into Claude Code.
#
# The release is public, so no account, token or GitHub client is needed:
# every download is an anonymous request to the release's download URL.
# COMMONMEASURE_RELEASE_URL replaces the release location with another origin
# that serves the same asset names under the same URL shape, for a mirror or
# a test; it defaults to the public repository's releases.
#
# What it does not do, and says when it matters: it does not edit shell
# profiles (when the install directory is not on PATH it prints the line to
# add), it does not build from source, and it does not install on a platform
# the release has no binary for; then it names the binaries the release holds.
#
# Exit status: 0 installed; 1 a download failed verification, or the installed
# binary did not report the release version, and nothing was left in place;
# 2 the installer cannot proceed on this machine, and the message names why.
set -eu

releases=${COMMONMEASURE_RELEASE_URL:-https://github.com/commonmeasure/commonmeasure/releases}
tag=""
dir="$HOME/.local/bin"
plugin=""

usage() {
  echo "usage: sh install.sh [--tag vX.Y.Z] [--dir DIR] [--plugin DIR]"
}

while [ $# -gt 0 ]; do
  case "$1" in
    --tag)    tag=$2; shift 2 ;;
    --dir)    dir=$2; shift 2 ;;
    --plugin) plugin=$2; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "commonmeasure installer: unknown argument $1" >&2; usage >&2; exit 2 ;;
  esac
done

cannot() { echo "commonmeasure installer: cannot $1" >&2; exit 2; }
fail()   { echo "commonmeasure installer: $1" >&2; exit 1; }

command -v curl >/dev/null 2>&1 || cannot "download: curl is not installed"
if command -v sha256sum >/dev/null 2>&1; then
  checksum() { sha256sum -c -; }
elif command -v shasum >/dev/null 2>&1; then
  checksum() { shasum -a 256 -c -; }
else
  cannot "verify a checksum: neither sha256sum nor shasum is installed"
fi
[ -z "$plugin" ] || command -v tar >/dev/null 2>&1 || cannot "unpack the plugin archive: tar is not installed"

os=$(uname -s)
arch=$(uname -m)
asset=""
case "$os" in
  Darwin)
    case "$arch" in
      arm64)          asset=commonmeasure-darwin-arm64 ;;
      x86_64)         asset=commonmeasure-darwin-x64 ;;
    esac ;;
  Linux)
    case "$arch" in
      x86_64)         asset=commonmeasure-linux-x64 ;;
      aarch64|arm64)  asset=commonmeasure-linux-arm64 ;;
    esac ;;
  MINGW*|MSYS*|CYGWIN*)
    asset=commonmeasure-win-x64.exe ;;
esac
[ -n "$asset" ] || cannot "install on $os/$arch: no release binary is named for it"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# Without --tag, the release location answers `latest` with a redirect to the
# page of the newest release, and the tag is the last part of that address.
# The redirect is read without being followed, so no page is downloaded.
if [ -z "$tag" ]; then
  location=$(curl -fsS -o /dev/null -w '%{redirect_url}' "$releases/latest") \
    || cannot "find the latest release at $releases/latest"
  case "$location" in
    */releases/tag/v*) tag=${location##*/releases/tag/} ;;
    *) cannot "find the latest release: $releases/latest did not point at a release tag. Pass --tag" ;;
  esac
fi
version=${tag#v}

# An asset is downloaded from the release's download address, following the
# redirect a release answers with. The address carries no credential.
download() {
  curl -fsSL "$releases/download/$tag/$1" -o "$tmp/$1"
}

download SHA256SUMS \
  || cannot "read release $tag at $releases/download/$tag/SHA256SUMS: the release may not exist, or it may hold no checksum list"

# SHA256SUMS names every asset the release holds, so it says whether this
# platform's binary is there before anything larger is downloaded.
held() {
  sed -n 's/^[0-9a-f]* *\(commonmeasure-[^ ]*\)$/\1/p' "$tmp/SHA256SUMS" | grep -v '\.tar\.gz$' | tr '\n' ' '
}
if ! grep -q " $asset\$" "$tmp/SHA256SUMS"; then
  cannot "install on $os/$arch: release $tag has no $asset. The binaries it holds: $(held)"
fi

fetch() {
  download "$1" || fail "downloading $1 from release $tag failed"
}

verify() {
  line=$(grep " $1\$" "$tmp/SHA256SUMS") || fail "SHA256SUMS in release $tag lists no entry for $1"
  ( cd "$tmp" && printf '%s\n' "$line" | checksum ) >/dev/null 2>&1 \
    || fail "$1 does not match its SHA-256 in SHA256SUMS; nothing was installed"
}

fetch "$asset"
verify "$asset"

mkdir -p "$dir" || cannot "create $dir"
target="$dir/commonmeasure"
case "$asset" in *.exe) target="$target.exe" ;; esac
chmod +x "$tmp/$asset"
# Moving over a running binary replaces the name and leaves the running inode
# alone, where copying over it fails.
mv "$tmp/$asset" "$target"

reported=$("$target" --version 2>/dev/null) || {
  rm -f "$target"
  fail "$target does not run on this machine and was removed"
}
[ "$reported" = "commonmeasure $version" ] || {
  rm -f "$target"
  fail "the installed binary reports '$reported' but the release is $tag; it was removed"
}
echo "installed $target: $reported, checksum verified"
case ":$PATH:" in
  *":$dir:"*) ;;
  *)
    echo "$dir is not on PATH. This installer does not edit shell profiles; add this line to yours:"
    echo "  export PATH=\"$dir:\$PATH\""
    ;;
esac

if [ -n "$plugin" ]; then
  archive="commonmeasure-plugin-$version.tar.gz"
  fetch "$archive"
  verify "$archive"
  mkdir -p "$plugin" || cannot "create $plugin"
  tar xzf "$tmp/$archive" -C "$plugin"
  echo "unpacked $archive into $plugin/commonmeasure-plugin, checksum verified. Install it into Claude Code with:"
  echo "  cd \"$plugin/commonmeasure-plugin\" && claude plugin marketplace add ./ && claude plugin install commonmeasure@commonmeasure"
fi

echo "Next: commonmeasure install claude (or codex, pi) to register with your host, then work a session, then run 'commonmeasure session' to see what it recorded and 'commonmeasure serve' for the console on loopback. Nothing leaves this machine."
