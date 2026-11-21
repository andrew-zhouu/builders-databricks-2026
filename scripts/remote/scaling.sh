#!/usr/bin/env bash
# Runs ON the remote machine: how do the sections scale with threads?
# 1 thread; 4 threads on 4 cores; 8 threads on 4 cores (2 per core).
# Prints ns/row per section (thread CPU time) and the wall time.
# Usage: scaling.sh [DATASET] [BINARY_NAME_IN_bin]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="${1:-m5}"; which="${2:-}"
BIN="$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions"; [ -n "$which" ] && BIN="$PROJ/bin/$which"
out=/tmp/ss-scaling; cat data/"$name"/*/*.csv >/dev/null
for cfg in "1 0" "4 0-3" "8 0-7"; do
  set -- $cfg
  rm -rf "$out"; mkdir -p "$out"
  NCPU="$1" SS_TIMING=1 taskset -c "$2" "$BIN" "data/$name" "$out" 2>"$out.log"
  echo "== $name, $1 thread(s) on cpus $2: wall $(grep 'done;' "$out.log" | sed 's/^\[ *\([0-9.]*\) ms.*/\1/') ms"
  awk '/^    (parse|dedup|sessions|links|write)/ { n=$1; i=2; if ($2 !~ /^[0-9.]+$/) { n=n" "$2; i=3 }; printf "   %-18s cpu %8.1f ms  %6.1f ns/row\n", n, $i, $(i+5) }' "$out.log"
done
rm -rf "$out" "$out.log"
