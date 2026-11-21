#!/usr/bin/env bash
# Benchmark only if the machine is quiet enough to mean something.
# Usage: quiet.sh DATASET [RUNS] [CPUSET]   (default cpuset 0-7 = 4 cores x 2 hyperthreads)
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
name="${1:-m20}"; runs="${2:-5}"; cpus="${3:-0-7}"
busy() { # percent of CPU time not idle over one second
  read -r _ u1 n1 s1 i1 w1 q1 sq1 _ < /proc/stat; sleep 1
  read -r _ u2 n2 s2 i2 w2 q2 sq2 _ < /proc/stat
  local total=$(( (u2+n2+s2+i2+w2+q2+sq2) - (u1+n1+s1+i1+w1+q1+sq1) ))
  echo $(( 100 - 100 * (i2 - i1) / total ))
}
b=$(busy)
avail=$(awk '/MemAvailable/ {print int($2/1024)}' /proc/meminfo)
echo "cpu busy ${b}% of 16 threads, ${avail} MB available"
if [ "$b" -gt "${MAX_BUSY:-12}" ] || [ "$avail" -lt "${MIN_AVAIL:-5000}" ]; then
  echo "NOT QUIET: skipping benchmark"; exit 2
fi
out="/tmp/ss-quiet-$$"
cat "$PROJ"/data/"$name"/*/*.csv > /dev/null   # warm the page cache
for i in $(seq "$runs"); do
  rm -rf "$out"; mkdir -p "$out"
  s=$(date +%s%N)
  NCPU=8 TIMING=1 taskset -c "$cpus" "$BIN" "$PROJ/data/$name" "$out" 2>"$out.log"
  e=$(date +%s%N)
  echo "run $i: wall $(( (e - s) / 1000000 )) ms   $(grep -E 'people resolved|events and clicks|deduplicated|sessions built' "$out.log" | sed 's/^\[ *\([0-9.]*\) ms\] \(.*\)/\2 @\1/' | tr '\n' ';')  busy now $(busy)%"
done
grep -E 'cpu |done' "$out.log"
rm -rf "$out" "$out.log"
