#!/usr/bin/env bash
set -euo pipefail

if ! command -v gh >/dev/null 2>&1; then
  echo "Missing gh CLI" >&2
  exit 1
fi

TEMPLATE=.github/PULL_REQUEST_TEMPLATE.md
if [[ ! -f "$TEMPLATE" ]]; then
  echo "Missing PR template at $TEMPLATE" >&2
  exit 1
fi

TMP_FILE=${PR_BODY_PATH:-"/tmp/pr-$(date +%s).md"}
cp "$TEMPLATE" "$TMP_FILE"

if [[ -n "${EDITOR:-}" ]]; then
  "$EDITOR" "$TMP_FILE"
else
  echo "Edit PR body at: $TMP_FILE" >&2
fi

exec gh pr create --body-file "$TMP_FILE"
