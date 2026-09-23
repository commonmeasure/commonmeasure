#!/bin/sh
# Rebuild the prebuilt binaries this plugin bundles, for every supported
# platform, into plugin/bin/. Run from anywhere; paths resolve relative to this
# script.
#
# macOS targets build natively. Linux and Windows targets cross-compile with
# zig, so there are no platform toolchains to install; one command installs
# the tool with its own zig:
#   uv tool install cargo-zigbuild --with ziglang
# On Linux the machine's own platform also goes through zig when it is
# present, because that yields a static musl binary that runs on any Linux;
# without it the native build is linked against this machine's glibc and runs
# only where that glibc version or newer is present.
#
# The binaries are build outputs and are gitignored. A missing toolchain skips
# that target with a note rather than failing the whole run.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo=$(CDPATH= cd -- "$here/.." && pwd)
bin="$here/bin"
mkdir -p "$bin"

native() {
  target=$1; out=$2
  rustup target add "$target" >/dev/null 2>&1 || true
  echo "building $out ($target, native)"
  ( cd "$repo" && cargo build --release -p commonmeasure-cli --target "$target" )
  # Stage then move: a live session may be executing the previous binary, and
  # cp over a running executable fails with "text file busy" where mv replaces
  # the name and leaves the running inode alone.
  cp "$repo/target/$target/release/commonmeasure" "$bin/$out.staged"
  chmod +x "$bin/$out.staged"
  mv "$bin/$out.staged" "$bin/$out"
}

cross() {
  target=$1; out=$2
  if ! command -v cargo-zigbuild >/dev/null 2>&1; then
    echo "skip $out: cargo-zigbuild not installed (uv tool install cargo-zigbuild --with ziglang)" >&2
    return 0
  fi
  # A cargo-zigbuild installed as a uv tool carries its zig in its own
  # environment, reachable through the python beside the resolved binary;
  # the tool looks for zig through the python it is told about.
  if [ -z "${CARGO_ZIGBUILD_PYTHON_PATH:-}" ]; then
    resolved=$(readlink -f "$(command -v cargo-zigbuild)")
    [ -x "$(dirname "$resolved")/python" ] && export CARGO_ZIGBUILD_PYTHON_PATH="$(dirname "$resolved")/python"
  fi
  rustup target add "$target" >/dev/null 2>&1 || true
  echo "building $out ($target, zig cross)"
  if ! ( cd "$repo" && cargo zigbuild --release -p commonmeasure-cli --target "$target" ); then
    echo "skip $out: cross-compilation for $target failed on this host" >&2
    return 0
  fi
  src="$repo/target/$target/release/commonmeasure"
  [ -f "$src" ] || src="$src.exe"
  cp "$src" "$bin/$out.staged"
  chmod +x "$bin/$out.staged" 2>/dev/null || true
  mv "$bin/$out.staged" "$bin/$out"
}

case "$(uname -s)" in
  Darwin)
    native aarch64-apple-darwin       commonmeasure-darwin-arm64
    native x86_64-apple-darwin        commonmeasure-darwin-x64
    cross  x86_64-unknown-linux-musl  commonmeasure-linux-x64
    cross  aarch64-unknown-linux-musl commonmeasure-linux-arm64
    ;;
  *)
    case "$(uname -m)" in
      x86_64)
        own=x86_64-unknown-linux-musl;  own_out=commonmeasure-linux-x64
        other=aarch64-unknown-linux-musl; other_out=commonmeasure-linux-arm64 ;;
      *)
        own=aarch64-unknown-linux-musl; own_out=commonmeasure-linux-arm64
        other=x86_64-unknown-linux-musl;  other_out=commonmeasure-linux-x64 ;;
    esac
    rm -f "$bin/$own_out"
    if command -v cargo-zigbuild >/dev/null 2>&1; then
      cross "$own" "$own_out"
    fi
    if [ ! -f "$bin/$own_out" ]; then
      echo "note: $own_out is linked against this machine's glibc; install cargo-zigbuild for a static build" >&2
      native "$(rustc -vV | sed -n 's/^host: //p')" "$own_out"
    fi
    # The other Linux architecture and Windows cross-compile with zig from
    # here. macOS does not: the darwin link needs Apple's CoreFoundation
    # framework (chrono's timezone lookup), which only a macOS SDK provides —
    # darwin binaries are built on a Mac, where this script's Darwin branch
    # builds every platform.
    cross "$other" "$other_out"
    ;;
esac
cross x86_64-pc-windows-gnu commonmeasure-win-x64.exe

echo "done. bundled binaries:"
ls -1 "$bin" | grep -v commonmeasure-launch
