#!/usr/bin/env bash
# Time the fast binary on a dataset with phase breakdown.
# Usage: timeit.sh DATASET [NCPU] [RUNS] [CPUSET]
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
name="${1:-dev}"; ncpu="${2:-8}"; runs="${3:-3}"; cpus="${4:-}"
out="$PROJ/out/time-$name"
pin=""
if [ -n "$cpus" ]; then pin="taskset -c $cpus"; fi
for i in $(seq "$runs"); do
  rm -rf "$out"; mkdir -p "$out"
  s=$(date +%s%N)
  NCPU="$ncpu" TIMING=1 $pin "$BIN" "$PROJ/data/$name" "$out" 2>"$out.log"
  e=$(date +%s%N)
  echo "run $i: wall $(( (e - s) / 1000000 )) ms (ncpu=$ncpu ${cpus:+cpus=$cpus})"
  if [ "$i" = "$runs" ]; then cat "$out.log"; fi
done
