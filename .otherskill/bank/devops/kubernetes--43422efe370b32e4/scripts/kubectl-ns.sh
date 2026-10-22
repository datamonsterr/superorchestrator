#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 2 ]]; then
  echo "Usage: kubectl-ns.sh jangar get pods" >&2
  exit 1
fi

NS="$1"
shift

exec kubectl -n "$NS" "$@"
