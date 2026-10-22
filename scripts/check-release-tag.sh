#!/bin/sh
# Release gate: the tag must be `v` + the workspace version in Cargo.toml, and CHANGELOG.md must
# have a section for that version. Usage: scripts/check-release-tag.sh TAG
set -eu

[ $# -eq 1 ] || { echo "usage: $0 TAG" >&2; exit 2; }
tag=$1
root=$(cd "$(dirname "$0")/.." && pwd)

version=$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$root/Cargo.toml" | head -n 1)
[ -n "$version" ] || { echo "release-check: no workspace version in Cargo.toml" >&2; exit 1; }

if [ "$tag" != "v$version" ]; then
  echo "release-check: tag '$tag' does not match Cargo.toml version (expected v$version)" >&2
  exit 1
fi

if ! grep -Eq "^## \[$(printf '%s' "$version" | sed 's/[.]/\\./g')\]" "$root/CHANGELOG.md"; then
  echo "release-check: CHANGELOG.md has no '## [$version]' section" >&2
  exit 1
fi
echo "release-check: ok ($tag)"
