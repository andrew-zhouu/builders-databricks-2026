#!/usr/bin/env bash
# Sampling profile by source function and line, using the binary's built-in
# perf-counter sampler (fast/src/prof.rs). All threads, full speed.
#
# Usage: perfprof.sh EVENT [DATASET] [NCPU] [LABEL]
#   EVENT: cycles | instructions | branch-misses | cache-misses  (optionally :period)
# Writes bench/prof-LABEL-EVENT.txt
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
set -euo pipefail
event="${1:-cycles}"; name="${2:-m5}"; ncpu="${3:-8}"; label="${4:-cur}"
mkdir -p "$PROJ/bench"
# Same code as the release build, plus line tables.
(cd "$PROJ/fast" && CARGO_PROFILE_RELEASE_DEBUG=line-tables-only \
  cargo build --release --target-dir "$PROJ/.toolchain/prof-target" 2>&1 | grep -E '^error' || true)
PBIN="$PROJ/.toolchain/prof-target/x86_64-unknown-linux-musl/release/sessions"
OBJDUMP=$(find "$RUSTUP_HOME/toolchains" -name llvm-objdump | head -1)
out="/tmp/ss-perfprof-$$"; rm -rf "$out"; mkdir -p "$out"
ev=$(cat "$PROJ/data/$name"/events/*.csv | wc -l)
SS_TIMING=1 SS_PROF="$event" SS_PROF_OUT="$out/hist.txt" NCPU="$ncpu" "$PBIN" "$PROJ/data/$name" "$out/o" 2>"$out/log.txt"
if [ ! -f "$PROJ/bench/prof-disasm.txt" ] || [ "$PBIN" -nt "$PROJ/bench/prof-disasm.txt" ]; then
  "$OBJDUMP" -d -l --no-show-raw-insn "$PBIN" > "$PROJ/bench/prof-disasm.txt"
fi
f="$PROJ/bench/prof-$label-${event%%:*}.txt"
{ echo "# $event samples, $name, ncpu=$ncpu, load $(cut -d' ' -f1 /proc/loadavg)"
  grep -E 'section|parse|dedup|sessions|links|write|resolve' "$out/log.txt"
  python3 -I "$PROJ/tools/icount_report.py" "$out/hist.txt" "$PROJ/bench/prof-disasm.txt" "$PROJ/fast/src" 0 0 0; } > "$f"
rm -rf "$out"
echo "written: $f"; sed -n '1,200p' "$f" | head -"${LINES_SHOWN:-95}"
