#!/usr/bin/env bash
# Interleaved A/B of whole runs, any binaries: wall time, user-mode cycles and
# instructions (tools/pstat.rs), user and system time. Reports min and median.
#
# Usage: abx.sh DATASET ROUNDS VARIANT...
#   VARIANT = comma-separated env assignments; SSBIN=name runs bin/name (snap.sh),
#   otherwise the current build. NCPU (default 8) and PIN (cpu list for taskset,
#   e.g. PIN=0-7 = 4 cores x 2 hyperthreads) apply to all variants.
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
mkdir -p "$PROJ/bin"
if [ ! -x "$PROJ/bin/pstat" ] || [ "$PROJ/tools/pstat.rs" -nt "$PROJ/bin/pstat" ]; then
  rustc -O --edition 2021 --target x86_64-unknown-linux-musl -C linker=rust-lld "$PROJ/tools/pstat.rs" -o "$PROJ/bin/pstat" 2>/dev/null
fi
name="$1"; rounds="$2"; shift 2
out="/tmp/ss-abx-$$"; res="/tmp/ss-abx-$$.res"; : > "$res"
cat "$PROJ"/data/"$name"/*/*.csv > /dev/null   # warm the page cache
for r in $(seq "$rounds"); do
  for v in "$@"; do
    envs="${v//,/ }"; bin="$BIN"
    for kv in $envs; do case "$kv" in SSBIN=*) bin="$PROJ/bin/${kv#SSBIN=}";; esac; done
    rm -rf "$out"; mkdir -p "$out"
    line=$(env $envs NCPU="${NCPU:-8}" ${PIN:+taskset -c $PIN} "$PROJ/bin/pstat" "$bin" "$PROJ/data/$name" "$out" 2>&1 >/dev/null | tail -1)
    echo "$v $line" >> "$res"
  done
done
rm -rf "$out"
printf '%-30s %13s %13s %13s %15s %11s\n' variant "wall ms" "user ms" "sys ms" "user Mcycles" "Minstr"
for v in "$@"; do
  awk -v v="$v" '$1==v' "$res" > "$res.v"
  stat() { awk -v c="$1" '{print $c}' "$res.v" | sort -n | awk '{a[NR]=$1} END{printf "%.0f/%.0f", a[1], a[int((NR+1)/2)]}'; }
  printf '%-30s %13s %13s %13s %15s %11s\n' "$v" "$(stat 2)" "$(stat 3)" "$(stat 4)" "$(stat 5)" "$(stat 6)"
done
rm -f "$res" "$res.v"
echo "(min/median of $rounds interleaved rounds; ncpu=${NCPU:-8}${PIN:+ pinned to $PIN}; load $(cut -d' ' -f1 /proc/loadavg))"
