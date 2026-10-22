#!/usr/bin/env bash
# End-to-end smoke test of a built superorchestrator binary. No real harness CLI is run:
# the config dir, socket and HOME are throwaway, and the daemon is started on demand by `ui`.
#
# Usage: scripts/smoke-ci.sh [PATH_TO_BINARY]
#   Without an argument it builds the release binary (`cargo build --release -p superorchestrator`)
#   and locates it through `cargo metadata`. Build web/dist first so the UI is embedded.
#   SMOKE_ALLOW_PLACEHOLDER=1 accepts the "web UI is not built" placeholder page.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

if [ $# -ge 1 ]; then
  bin=$1
else
  cargo build --release --locked -p superorchestrator
  target_dir=$(cargo metadata --no-deps --format-version 1 | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
  bin=$target_dir/release/superorchestrator
fi
[ -x "$bin" ] || { echo "smoke: binary not found: $bin" >&2; exit 1; }

tmp=$(mktemp -d "${TMPDIR:-/tmp}/so-smoke.XXXXXX")
export HOME=$tmp/home
export SUPERORCHESTRATOR_CONFIG_DIR=$tmp/config
export SUPERORCHESTRATOR_SOCKET=$tmp/s.sock
unset XDG_RUNTIME_DIR XDG_CONFIG_HOME XDG_DATA_HOME
mkdir -p "$HOME"

cleanup() {
  local pid
  pid=$("$bin" --json status 2>/dev/null | sed -n 's/.*"pid": *\([0-9][0-9]*\).*/\1/p' | head -n 1 || true)
  if [ -n "${pid:-}" ]; then kill "$pid" 2>/dev/null || true; fi
  rm -rf "$tmp"
}
trap cleanup EXIT

fail() { echo "smoke: FAIL: $*" >&2; exit 1; }
step() { echo "smoke: $*"; }

want=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n 1)
step "version (expect $want)"
got=$("$bin" --version)
[ "$got" = "superorchestrator $want" ] || fail "--version printed '$got'"

step "init"
"$bin" init --yes >/dev/null
[ -f "$SUPERORCHESTRATOR_CONFIG_DIR/config.toml" ] || fail "init did not write config.toml"

step "doctor"
"$bin" doctor >/dev/null || fail "doctor failed"

step "status (no supervisor yet)"
"$bin" status | grep -q "supervisor: not running" || fail "status should report no supervisor"

step "ui --no-open --port 0"
url=$("$bin" --json ui --no-open --port 0 | sed -n 's/.*"url": *"\([^"]*\)".*/\1/p')
[ -n "$url" ] || fail "ui printed no url"
url=${url%/}
step "ui at $url"

step "GET /api/v1/status"
api=$(curl --fail --silent --show-error --max-time 10 "$url/api/v1/status") || fail "/api/v1/status failed"
case "$api" in *'"'*) ;; *) fail "/api/v1/status is not JSON: $api" ;; esac

step "GET / (embedded index.html)"
page=$(curl --fail --silent --show-error --max-time 10 "$url/") || fail "GET / failed"
case "$page" in *"<html"*|*"<!doctype"*|*"<!DOCTYPE"*) ;; *) fail "GET / is not HTML" ;; esac
if printf '%s' "$page" | grep -q "web UI is not built"; then
  [ "${SMOKE_ALLOW_PLACEHOLDER:-0}" = 1 ] || fail "binary embeds the placeholder UI; build web/dist first"
fi
printf '%s' "$page" | grep -q 'id="root"' || [ "${SMOKE_ALLOW_PLACEHOLDER:-0}" = 1 ] \
  || fail "index.html has no #root mount point"

step "status (supervisor running)"
"$bin" status | grep -q "supervisor: running" || fail "status should report a running supervisor"

echo "smoke: ok"
