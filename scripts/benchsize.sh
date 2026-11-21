#!/usr/bin/env bash
# Parser micro-benchmark over region sizes: separates compute from memory effects.
# Usage: benchsize.sh [DATASET]
. /home/andre/builders/databricks/scripts/env.sh
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
name="${1:-dev}"
for spec in "262144 400" "1048576 200" "4194304 60" "20000000 12"; do
  set -- $spec
  printf 'region %9s: ' "$1"
  BENCH_BYTES="$1" BENCH_ITERS="$2" NCPU=1 "$BIN" --bench "$PROJ/data/$name" /tmp/ss-bench-out 2>&1 | grep '^bench' | sed 's/ (.*//' | tr '\n' ';'
  echo " load $(cut -d' ' -f1 /proc/loadavg)"
done
