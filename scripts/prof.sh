#!/usr/bin/env bash
# Per-section thread-CPU profile (TIMING=1) of the fast binary on a dataset.
# Usage: [ENV=..] prof.sh DATASET [NCPU]
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
name="${1:-dev}"; ncpu="${2:-8}"
out="/tmp/ss-prof-$$"; rm -rf "$out"; mkdir -p "$out"
echo "== $name ncpu=$ncpu load $(cut -d' ' -f1 /proc/loadavg) ${EXTRA:-}"
NCPU="$ncpu" TIMING=1 /usr/bin/time -f "   wall %es user %Us sys %Ss rss %MKB" "$BIN" "$PROJ/data/$name" "$out" 2>&1 | grep -v '^\[.*setup'
rm -rf "$out"
