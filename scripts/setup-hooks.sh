#!/usr/bin/env bash
# Point git at the tracked hooks in .githooks.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
git config core.hooksPath .githooks
chmod +x .githooks/* scripts/*.sh
echo "hooks installed (core.hooksPath=.githooks)"
