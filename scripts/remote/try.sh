#!/usr/bin/env bash
# Runs ON the remote machine: build the pushed sources and run the quick checks.
# Usage: try.sh [FUZZ_COUNT] [FUZZ_FIRST]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
out=$( (cd fast && cargo build --release 2>&1) )
if ! echo "$out" | grep -q 'Finished'; then echo "$out" | grep -E '^(error|warning: unused)' -A12 | head -80; exit 1; fi
echo "$out" | grep -E '^warning' -A6 | head -30
bash scripts/check.sh 2>&1 | tail -4 || exit 1
n="${1:-0}"
if [ "$n" -gt 0 ]; then
  bash scripts/fuzz.sh "${2:-1}" "$n" 3000 2>&1 | tail -6
fi
