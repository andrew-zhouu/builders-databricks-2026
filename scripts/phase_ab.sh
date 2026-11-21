#!/usr/bin/env bash
# Interleaved A/B of a phase timestamp (ms since start, from TIMING output).
# Usage: phase_ab.sh DATASET "PHASE LABEL REGEX" ROUNDS VARIANT...
# A variant is a space-free env assignment list separated by commas, e.g.
#   X=1                 (no-op: the current build)
#   SSBIN=old           (the binary saved as bin/old by snap.sh)
#   SSBIN=old,NOZAP=1
# Prints min and median of the timestamp for each variant. Interleaving makes
# the comparison fair on a machine whose load drifts.
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
name="$1"; label="$2"; rounds="$3"; shift 3
out="/tmp/ss-phase-$$"
declare -A vals
for r in $(seq "$rounds"); do
  for v in "$@"; do
    envs="${v//,/ }"
    bin="$BIN"
    for kv in $envs; do case "$kv" in SSBIN=*) bin="$PROJ/bin/${kv#SSBIN=}";; esac; done
    rm -rf "$out"; mkdir -p "$out"
    t=$(env $envs NCPU="${NCPU:-8}" TIMING=1 "$bin" "$PROJ/data/$name" "$out" 2>&1 | grep -E "$label" | head -1 | sed 's/^\[ *\([0-9.]*\) ms.*/\1/')
    vals[$v]="${vals[$v]:-} $t"
  done
done
rm -rf "$out"
for v in "$@"; do
  echo "${vals[$v]}" | tr ' ' '\n' | grep . | sort -n | awk -v v="$v" '{a[NR]=$1} END{printf "%-28s min %8.2f  median %8.2f  (n=%d)\n", v, a[1], a[int((NR+1)/2)], NR}'
done
echo "(load $(cut -d' ' -f1 /proc/loadavg))"
