#!/usr/bin/env bash
# Full local gate: mirrors CI. Usage: scripts/check.sh [--rust-only|--web-only]
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

run_rust=1
run_web=1
case "${1:-}" in
  "") ;;
  --rust-only) run_web=0 ;;
  --web-only) run_rust=0 ;;
  *) echo "usage: $0 [--rust-only|--web-only]" >&2; exit 2 ;;
esac

if [ "$run_rust" = 1 ]; then
  cargo fmt --all --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
fi

if [ "$run_web" = 1 ]; then
  pnpm -C web install --frozen-lockfile
  pnpm -C web lint
  pnpm -C web typecheck
  pnpm -C web test
  pnpm -C web build
fi

echo "check: ok"
