#!/usr/bin/env bash
# Interleaved A/B of the events parser micro-benchmark on a large region
# (memory-realistic). Variants are env assignments, e.g.:
#   ab.sh dev "PREFETCH=0" "PREFETCH=512" "PREFETCH=1024"
# Prints the best ns/line of each variant over ROUNDS interleaved rounds.
. /home/andre/builders/databricks/scripts/env.sh
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
name="$1"; shift
rounds="${ROUNDS:-5}"; bytes="${BENCH_BYTES:-20000000}"; iters="${BENCH_ITERS:-8}"
declare -A best
for r in $(seq "$rounds"); do
  for v in "$@"; do
    x=$(env $v BENCH_BYTES="$bytes" BENCH_ITERS="$iters" NCPU=1 "$BIN" --bench "$PROJ/data/$name" /tmp/ss-bench-out 2>&1 \
        | grep "bench ${KIND:-events}" | sed 's/.*: \([0-9.]*\) ns.*/\1/')
    if [ -z "${best[$v]:-}" ] || awk "BEGIN{exit !($x < ${best[$v]})}"; then best[$v]=$x; fi
  done
done
for v in "$@"; do printf '%-28s %6s ns/line\n' "$v" "${best[$v]}"; done
echo "(load $(cut -d' ' -f1 /proc/loadavg))"
