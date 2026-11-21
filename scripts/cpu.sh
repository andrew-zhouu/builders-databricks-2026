#!/usr/bin/env bash
# CPU-time benchmark (robust to a busy machine): runs the binary N times and
# prints wall/user/sys per run plus the best of each.
# Usage: [ENV=..] cpu.sh DATASET [NCPU] [RUNS]
. /home/andre/builders/databricks/scripts/env.sh
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
name="${1:-dev}"; ncpu="${2:-8}"; runs="${3:-5}"
out="/tmp/ss-cpu-$$"; log="/tmp/ss-cpu-$$.txt"; : >"$log"
for i in $(seq "$runs"); do
  rm -rf "$out"; mkdir -p "$out"
  NCPU="$ncpu" /usr/bin/time -a -o "$log" -f "%e %U %S %M" "$BIN" "$PROJ/data/$name" "$out" 2>/dev/null
done
rm -rf "$out"
awk -v n="$name" -v c="$ncpu" '{ if (NR==1||$1<w) w=$1; if (NR==1||$2<u) u=$2; if (NR==1||$3<s) s=$3; if (NR==1||$2+$3<t) t=$2+$3; m=$4 }
  END { printf "%-8s ncpu=%-2s best wall %.2fs  user %.2fs  sys %.2fs  cpu %.2fs  rss %dMB  (load %s)\n", n, c, w, u, s, t, m/1024, L }' L="$(cut -d' ' -f1 /proc/loadavg)" "$log"
rm -f "$log"
