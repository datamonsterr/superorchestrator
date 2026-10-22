#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "Usage: find-in-repo.sh Temporal services" >&2
  exit 1
fi

PATTERN="$1"
PATH_ARG=${2:-.}

exec rg -n "$PATTERN" "$PATH_ARG"
