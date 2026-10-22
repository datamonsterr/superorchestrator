#!/usr/bin/env bash
set -euo pipefail

ADDRESS=${TEMPORAL_ADDRESS:-}
NAMESPACE=${TEMPORAL_NAMESPACE:-default}

if [[ $# -lt 1 ]]; then
  echo "Usage: temporal-run.sh workflow list --limit 5" >&2
  exit 1
fi

ARGS=("--namespace" "$NAMESPACE")
if [[ -n "$ADDRESS" ]]; then
  ARGS+=("--address" "$ADDRESS")
fi

exec temporal "${ARGS[@]}" "$@"
