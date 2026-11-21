#!/usr/bin/env bash
# Wait (up to MAX_WAIT seconds) for the machine to go quiet, then run the
# pinned benchmark once and append the result to bench/quiet-results.txt.
# Usage: when_quiet.sh DATASET [RUNS] [MAX_WAIT]
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
name="${1:-m20}"; runs="${2:-5}"; max="${3:-7200}"
mkdir -p "$PROJ/bench"
waited=0
while [ "$waited" -lt "$max" ]; do
  if out=$(bash "$PROJ/scripts/quiet.sh" "$name" "$runs" 2>&1); then
    { echo "=== $(date -u +%FT%TZ) $(git -C "$PROJ" rev-parse --short HEAD) $name"; echo "$out"; } | tee -a "$PROJ/bench/quiet-results.txt"
    exit 0
  fi
  sleep 20; waited=$((waited + 21))
done
echo "never quiet within ${max}s"; exit 2
