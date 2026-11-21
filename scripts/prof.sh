#!/usr/bin/env bash
# Per-section profile (SS_TIMING=1) of the fast binary on a dataset: CPU and
# kernel time, page faults, and instructions / cycles / misses per input row.
# Usage: [SS_...=..] prof.sh DATASET [NCPU] [CPUSET]       BIN=... picks another binary
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
name="${1:-dev}"; ncpu="${2:-8}"; cpus="${3:-}"
pin=""; [ -n "$cpus" ] && pin="taskset -c $cpus"
out="/tmp/ss-prof-$$"; rm -rf "$out"; mkdir -p "$out"
echo "== $name ncpu=$ncpu ${cpus:+cpus=$cpus }load $(cut -d' ' -f1 /proc/loadavg) avail $(awk '/MemAvailable/ {print int($2/1024)}' /proc/meminfo)MB $(env | grep '^SS_' | grep -v SS_TIMING | tr '\n' ' ')"
NCPU="$ncpu" SS_TIMING=1 /usr/bin/time -f "   wall %es user %Us sys %Ss rss %MKB minflt %R majflt %F" $pin "$BIN" "$PROJ/data/$name" "$out" 2>&1 | grep -v 'setup:' | cut -c1-140
rm -rf "$out"
