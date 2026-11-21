#!/usr/bin/env bash
# Runs ON the remote machine: the reference measurements of the scored binary
# and the current build, on a quiet 8-vCPU machine (4 cores x 2 hyperthreads).
# Usage: baseline.sh [ROUNDS]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
rounds="${1:-7}"
echo "== $(date -u +%FT%TZ) $(git rev-parse --short HEAD) $(lscpu | grep 'Model name' | sed 's/.*: *//') load $(cut -d' ' -f1 /proc/loadavg)"
for name in dev m5 m20 n20 m60; do
  [ -f "data/$name/manifest.json" ] || continue
  echo "-- $name"
  bash scripts/abx.sh "$name" "$rounds" SSBIN=scored X=1 | grep -v '^(min'
done
echo "-- per-section profile of the current build, m20"
bash scripts/prof.sh m20 8 | cut -c1-150
