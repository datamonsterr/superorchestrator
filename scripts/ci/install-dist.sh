#!/usr/bin/env bash
# Install the pinned cargo-dist (`dist`) release after verifying its published sha256.
#
# Used by .github/workflows/release.yml in the `plan` and `build-local-artifacts` jobs.
# Keep DIST_VERSION in sync with `cargo-dist-version` in dist-workspace.toml (enforced by
# scripts/test-release-gate.py) and refresh the hashes from the release's sha256.sum:
# https://github.com/axodotdev/cargo-dist/releases/download/v${DIST_VERSION}/sha256.sum
#
# Requires bash, curl, tar and xz.
set -euo pipefail

DIST_VERSION="0.32.0"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) archive=cargo-dist-x86_64-unknown-linux-gnu.tar.xz; sha256=eb52f9fae0d0506774e9f1801c1168f87fa2c87a45e2d64d3ae7c89401929946 ;;
  Linux-aarch64) archive=cargo-dist-aarch64-unknown-linux-gnu.tar.xz; sha256=d29bcffeb3f8b0c517b4ce0dd2470926ed5cb0bb29d78c6bdd5f88d76ee14a6a ;;
  Darwin-x86_64) archive=cargo-dist-x86_64-apple-darwin.tar.xz; sha256=6243464a8389e006b9256ee548bc795638f1a17113c1b6669c0e05ce89fd05c5 ;;
  Darwin-arm64) archive=cargo-dist-aarch64-apple-darwin.tar.xz; sha256=aa343b2ff78ec2981f17a65140250c5ad6062c74072163f68c5c2686d94763a7 ;;
  *)
    echo "unsupported host: $(uname -s)-$(uname -m)" >&2
    exit 1
    ;;
esac

tmpdir="${RUNNER_TEMP:-$(mktemp -d)}"
mkdir -p "$tmpdir"
archive_path="$tmpdir/$archive"

curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "$archive_path" \
  "https://github.com/axodotdev/cargo-dist/releases/download/v${DIST_VERSION}/${archive}"

if command -v sha256sum > /dev/null 2>&1; then
  printf '%s  %s\n' "$sha256" "$archive_path" | sha256sum -c -
else
  printf '%s  %s\n' "$sha256" "$archive_path" | shasum -a 256 -c -
fi

tar -xJf "$archive_path" -C "$tmpdir"
mkdir -p "$HOME/.cargo/bin"
install -m 755 "$tmpdir/${archive%.tar.xz}/dist" "$HOME/.cargo/bin/dist"

if [ -n "${GITHUB_PATH:-}" ]; then
  printf '%s\n' "$HOME/.cargo/bin" >> "$GITHUB_PATH"
fi
