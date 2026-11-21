#!/usr/bin/env bash
# Does a run actually get transparent huge pages? Diffs /proc/vmstat around one run.
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
name="${1:-m5}"
snap() { grep -E '^(thp_fault_alloc|thp_fault_fallback|thp_fault_fallback_charge|compact_stall|pgfault|pgmajfault) ' /proc/vmstat; }
cat /sys/kernel/mm/transparent_hugepage/enabled /sys/kernel/mm/transparent_hugepage/defrag
a=$(snap); out=/tmp/ss-thp-$$; mkdir -p "$out"
NCPU=8 "$BIN" "$PROJ/data/$name" "$out"; rm -rf "$out"
b=$(snap)
paste <(echo "$a") <(echo "$b") | awk '{printf "%-28s +%d\n", $1, $4-$2}'
