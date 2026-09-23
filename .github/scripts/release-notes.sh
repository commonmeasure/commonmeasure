#!/bin/sh
# Print the CHANGELOG.md section for one version, without its heading.
# Fails when the section is missing, empty or still marked unreleased, so a
# release is not cut without its notes.
set -eu
version=${1:?usage: release-notes.sh <version, without the v>}
changelog=${2:-CHANGELOG.md}
heading=$(grep -E "^## $(printf '%s' "$version" | sed 's/\./\\./g')( |$)" "$changelog" || true)
[ -n "$heading" ] || { echo "$changelog has no section for $version" >&2; exit 1; }
case $heading in
  *unreleased*) echo "$changelog still marks $version unreleased; give it the release date" >&2; exit 1 ;;
esac
body=$(awk -v h="$heading" '$0 == h {f=1; next} /^## /{f=0} f' "$changelog")
[ -n "$(printf '%s' "$body" | tr -d '[:space:]')" ] || { echo "the $version section of $changelog is empty" >&2; exit 1; }
printf '%s\n' "$body" | sed '1{/^$/d;}'
