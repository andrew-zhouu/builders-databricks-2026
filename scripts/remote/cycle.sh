#!/usr/bin/env bash
# Runs ON the remote machine: one development cycle for the pushed sources.
# Build, quick checks, a fuzz batch, then an interleaved A/B against the scored
# binary and the per-section profile.
# Usage: cycle.sh [FUZZ_COUNT] [FUZZ_FIRST] [DATASET] [ROUNDS]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
n="${1:-10}"; first="${2:-1}"; name="${3:-m20}"; rounds="${4:-5}"
bash scripts/remote/try.sh "$n" "$first" 2>&1 | tail -5
[ "${PIPESTATUS[0]}" = 0 ] || exit 1
bash scripts/abx.sh "$name" "$rounds" SSBIN=scored X=1 ${EXTRA_VARIANTS:-} 2>&1 | grep -vE '^variant' | cut -c1-75
bash scripts/prof.sh "$name" 8 2>&1 | grep -E 'parsed|deduplicated|sessions built|released|parse|dedup|sessions |links|wall' | cut -c1-78
