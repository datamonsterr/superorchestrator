#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "Usage: deploy-service.sh bumba" >&2
  echo "Services: bumba or jangar" >&2
  exit 1
fi

SERVICE="$1"
shift

case "$SERVICE" in
  bumba)
    exec bun run packages/scripts/src/bumba/deploy-service.ts "$@"
    ;;
  jangar)
    exec bun run packages/scripts/src/jangar/deploy-service.ts "$@"
    ;;
  *)
    echo "Unknown service: $SERVICE" >&2
    exit 1
    ;;
esac
