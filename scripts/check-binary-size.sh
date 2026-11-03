#!/bin/sh
# Fails when the release binary is larger than the budget.
# Usage: scripts/check-binary-size.sh BINARY [MAX_BYTES]
#
# Budget: measured 52,142,448 bytes (x86_64 Linux, CI release build with web/dist embedded, after
# level insights) plus about 7% headroom, rounded to 56,000,000. Raise it deliberately, in a reviewed change.
set -eu

[ $# -ge 1 ] || { echo "usage: $0 BINARY [MAX_BYTES]" >&2; exit 2; }
bin=$1
max=${2:-56000000}
[ -f "$bin" ] || { echo "size: not found: $bin" >&2; exit 1; }

size=$(wc -c < "$bin" | tr -d ' ')
echo "size: $bin is $size bytes (budget $max)"
if [ "$size" -gt "$max" ]; then
  echo "size: FAIL: binary exceeds the budget by $((size - max)) bytes" >&2
  exit 1
fi
