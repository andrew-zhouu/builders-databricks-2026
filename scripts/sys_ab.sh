#!/usr/bin/env bash
# Interleaved A/B of whole-process wall / user / sys time (includes exit teardown).
# Usage: sys_ab.sh DATASET ROUNDS VARIANT...   (variants as in phase_ab.sh)
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
name="$1"; rounds="$2"; shift 2
out="/tmp/ss-sys-$$"; log="/tmp/ss-sys-$$.txt"
declare -A W U S
for r in $(seq "$rounds"); do
  for v in "$@"; do
    envs="${v//,/ }"
    bin="$BIN"
    for kv in $envs; do case "$kv" in SSBIN=*) bin="$PROJ/bin/${kv#SSBIN=}";; esac; done
    rm -rf "$out"; mkdir -p "$out"
    env $envs NCPU="${NCPU:-8}" /usr/bin/time -o "$log" -f "%e %U %S" "$bin" "$PROJ/data/$name" "$out" 2>/dev/null
    read -r w u s < "$log"
    W[$v]="${W[$v]:-} $w"; U[$v]="${U[$v]:-} $u"; S[$v]="${S[$v]:-} $s"
  done
done
rm -rf "$out" "$log"
stat() { echo "$1" | tr ' ' '\n' | grep . | sort -n | awk '{a[NR]=$1} END{printf "%5.2f/%5.2f", a[1], a[int((NR+1)/2)]}'; }
printf '%-28s %-12s %-12s %-12s\n' variant "wall min/med" "user min/med" "sys min/med"
for v in "$@"; do
  printf '%-28s %-12s %-12s %-12s\n' "$v" "$(stat "${W[$v]}")" "$(stat "${U[$v]}")" "$(stat "${S[$v]}")"
done
echo "(load $(cut -d' ' -f1 /proc/loadavg))"
