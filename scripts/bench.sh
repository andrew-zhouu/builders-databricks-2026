#!/usr/bin/env bash
# Parser micro-benchmark: best-of-N thread CPU time per line (see main.rs bench()).
# Usage: bench.sh [DATASET] [REPEATS]
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
name="${1:-dev}"; reps="${2:-3}"
for i in $(seq "$reps"); do
  NCPU=1 "$BIN" --bench "$PROJ/data/$name" /tmp/ss-bench-out 2>&1 | grep '^bench' | tr '\n' ';'
  echo " load $(cut -d' ' -f1 /proc/loadavg)"
done
