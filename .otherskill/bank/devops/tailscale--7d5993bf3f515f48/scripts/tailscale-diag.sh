#!/usr/bin/env bash
set -euo pipefail

HOST=${1:-temporal-grpc.ide-newton.ts.net}

echo "== tailscale status =="
tailscale status

echo "== tailscale netcheck =="
tailscale netcheck || true

echo "== tailscale ping $HOST =="
tailscale ping "$HOST"
