#!/bin/sh
# Install the prebuilt `commonmeasure` binary for this platform from a public
# release of the product repository, github.com/commonmeasure/commonmeasure:
#
#   sh install.sh [--tag v0.3.1] [--dir DIR] [--plugin DIR] [--update]
#                 [--agree-reporting]
#
# A release holds one binary per supported platform, the plugin archive and
# SHA256SUMS over every asset. The installer picks the binary for this
# platform, downloads it with the checksum list, verifies the checksum before
# the binary is placed on PATH, and checks that the installed binary reports
# the release's version. Without --tag it installs the latest release.
# --dir chooses where the binary goes; the default is ~/.local/bin. --plugin
# also downloads and verifies the plugin archive, unpacks it into DIR and
# prints the commands that install it into Claude Code. --update leaves out
# the first-install next steps and the consent question; `commonmeasure
# update` runs this script, as embedded in the binary, with it.
#
# Reporting consent: some sources license their content only if each use is
# reported, and they are refused until the operator agrees to report to
# them. When the operator home records no answer, the installer shows the
# consent text and asks once, reading the answer from the terminal
# (/dev/tty, since `curl | sh` has no stdin). --agree-reporting, or
# COMMONMEASURE_REPORTING_CONSENT=agree in the environment, records
# agreement without asking, for an unattended install. Without a terminal
# and without either, nothing is recorded and the installer prints the
# command that agrees later: `commonmeasure consent agree`. A declined answer
# records nothing either. The answer is the binary's to record, in the
# operator home it resolves (COMMONMEASURE_HOME or its default).
#
# The release is public, so no account, token or GitHub client is needed:
# every download is an anonymous request to the release's download URL.
# COMMONMEASURE_RELEASE_URL replaces the release location with another origin
# that serves the same asset names under the same URL shape, for a mirror or
# a test; it defaults to the public repository's releases. A release build of
# `commonmeasure update` sets it to the public repository's releases whatever
# the environment holds.
#
# What it does not do, and says when it matters: it does not edit shell
# profiles (when the install directory is not on PATH it prints the line to
# add), it does not build from source, and it does not install on a platform
# the release has no binary for; then it names the binaries the release holds.
#
# Exit status: 0 installed; 1 a download failed verification, or the downloaded
# binary did not report the release version, and the binary already in place
# (if any) was left as it was;
# 2 the installer cannot proceed on this machine, and the message names why.
set -eu

releases=${COMMONMEASURE_RELEASE_URL:-https://github.com/commonmeasure/commonmeasure/releases}
tag=""
dir="$HOME/.local/bin"
plugin=""
update=""
agree_reporting=""
[ "${COMMONMEASURE_REPORTING_CONSENT:-}" = agree ] && agree_reporting=1

usage() {
  echo "usage: sh install.sh [--tag vX.Y.Z] [--dir DIR] [--plugin DIR] [--update] [--agree-reporting]"
}

while [ $# -gt 0 ]; do
  case "$1" in
    --tag)    tag=$2; shift 2 ;;
    --dir)    dir=$2; shift 2 ;;
    --plugin) plugin=$2; shift 2 ;;
    --update) update=1; shift ;;
    --agree-reporting) agree_reporting=1; shift ;;
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

# The version is checked before the binary is placed, so a failure leaves the
# binary already installed where it was.
chmod +x "$tmp/$asset"
reported=$("$tmp/$asset" --version 2>/dev/null) \
  || fail "the $asset in release $tag does not run on this machine; nothing was installed"
[ "$reported" = "commonmeasure $version" ] \
  || fail "the downloaded binary reports '$reported' but the release is $tag; nothing was installed"

mkdir -p "$dir" || cannot "create $dir"
target="$dir/commonmeasure"
case "$asset" in *.exe) target="$target.exe" ;; esac
# Moving over a running binary replaces the name and leaves the running inode
# alone, where copying over it fails. `mv` across file systems copies first,
# so the staged copy sits beside the target and the final step is a rename.
cp "$tmp/$asset" "$target.new.$$" && mv "$target.new.$$" "$target" \
  || { rm -f "$target.new.$$"; cannot "write $target"; }
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

# The binary just installed reads and records the consent, so the file's
# format and the text shown are the binary's own. Only a home that records
# no answer is asked; an unreadable file is reported, not overwritten.
consent_state=$("$target" consent show --json 2>/dev/null | sed -n 's/^ *"state": *"\([a-z_]*\)".*/\1/p')
agree_later="Sources whose licence demands usage reporting are refused until you agree: commonmeasure consent agree"
case "$consent_state" in
  not_given)
    if [ -n "$agree_reporting" ]; then
      "$target" consent agree || echo "commonmeasure installer: recording reporting consent failed; run: commonmeasure consent agree" >&2
    elif [ -n "$update" ]; then
      :
    elif (: </dev/tty) 2>/dev/null; then
      {
        echo
        "$target" consent show
        echo
        printf 'Agree to report to sources that require it? [y/N] '
      } >/dev/tty
      answer=""
      read -r answer </dev/tty || answer=""
      case "$answer" in
        y|Y|yes|YES|Yes) "$target" consent agree || echo "commonmeasure installer: recording reporting consent failed; run: commonmeasure consent agree" >&2 ;;
        *) echo "$agree_later" ;;
      esac
    else
      echo "$agree_later (or rerun the installer with --agree-reporting)"
    fi
    ;;
  unreadable)
    "$target" consent show | head -n 1
    ;;
esac

# The closing line states what can leave, by the consent now recorded:
# with it, reports of uses of sources that demand reporting go to the
# receiver relay.json names. A home that never recorded an answer has
# nothing reported until a receiver is named and a policy scope clears
# egress. A withdrawn or unreadable consent refuses the next demanding
# source, but a use admitted while consent was agreed carries that consent
# on its record and is still reported.
if [ -z "$update" ]; then
  consent_state=$("$target" consent show --json 2>/dev/null | sed -n 's/^ *"state": *"\([a-z_]*\)".*/\1/p')
  case "$consent_state" in
    agreed)
      boundary="Session records stay on this machine. With reporting consent agreed, each use of a source whose licence demands reporting is reported to the telemetry receiver relay.json names, once one is named." ;;
    not_given)
      boundary="Session records stay on this machine, and no use of a source is reported until relay.json names a telemetry receiver and a policy scope clears egress." ;;
    *)
      boundary="Session records stay on this machine. Without reporting consent agreed, sources whose licence demands reporting are refused; a use admitted while consent was agreed is still reported to the telemetry receiver relay.json names, and no other use is reported until a policy scope clears egress." ;;
  esac
  echo "Next: commonmeasure install claude (or codex, pi, claude-desktop, cursor, copilot, vscode, chrome) to register with your host, then work a session, then run 'commonmeasure session' to see what it recorded and 'commonmeasure serve' for the console on loopback. $boundary"
fi
