#!/usr/bin/env bash
# Where one parser's instructions (or cycles) go, by source function and line:
# samples the cache-resident micro-benchmark, where nothing stalls, so
# instruction samples are an accurate picture of instruction counts.
#
# Usage: benchprof.sh [KIND] [EVENT] [DATASET]     KIND = events|clicks|links
# Writes bench/benchprof-KIND-EVENT.txt
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
set -euo pipefail
kind="${1:-events}"; event="${2:-instructions}"; name="${3:-dev}"
mkdir -p "$PROJ/bench"
(cd "$PROJ/fast" && CARGO_PROFILE_RELEASE_DEBUG=line-tables-only \
  cargo build --release --target-dir "$PROJ/.toolchain/prof-target" 2>&1 | grep -E '^error' || true)
PBIN="$PROJ/.toolchain/prof-target/x86_64-unknown-linux-musl/release/sessions"
OBJDUMP=$(find "$RUSTUP_HOME/toolchains" -name llvm-objdump | head -1)
out="/tmp/ss-benchprof-$$"; rm -rf "$out"; mkdir -p "$out"
SS_PROF="$event:${PERIOD:-10007}" SS_PROF_OUT="$out/hist.txt" SS_BENCH_KIND="$kind" SS_BENCH_ITERS="${ITERS:-300}" NCPU=1 \
  "$PBIN" --bench "$PROJ/data/$name" "$out/o" 2>"$out/log.txt"
"$OBJDUMP" -d -l --no-show-raw-insn "$PBIN" > "$PROJ/bench/prof-disasm.txt"
f="$PROJ/bench/benchprof-$kind-${event}.txt"
{ grep '^bench' "$out/log.txt"
  python3 -I "$PROJ/tools/icount_report.py" "$out/hist.txt" "$PROJ/bench/prof-disasm.txt" "$PROJ/fast/src" 0 0 0; } > "$f"
rm -rf "$out"
echo "written: $f"
