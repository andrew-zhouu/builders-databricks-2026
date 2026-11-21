#!/usr/bin/env bash
# Instruction-count profile of the pipeline (single thread) on a small dataset.
# Deterministic: independent of machine load. See tools/icount.rs.
#
# Usage: icount.sh [LABEL] [DATASET]     writes bench/icount-LABEL.txt
#   env PROF_ENV="SS_X=1 ..." passes switches to the profiled run.
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
set -euo pipefail
label="${1:-cur}"; name="${2:-mini}"
mkdir -p "$PROJ/bin" "$PROJ/bench"
if [ ! -x "$PROJ/bin/icount" ] || [ "$PROJ/tools/icount.rs" -nt "$PROJ/bin/icount" ]; then
  rustc -O --edition 2021 --target x86_64-unknown-linux-musl -C linker=rust-lld \
    "$PROJ/tools/icount.rs" -o "$PROJ/bin/icount"
fi
if [ ! -f "$PROJ/data/mini/manifest.json" ]; then
  generate --seed 11 --events 20000 --clicks 5000 --output "$PROJ/data/mini" --threads 2
fi
# Same code as the release build, plus line tables.
(cd "$PROJ/fast" && CARGO_PROFILE_RELEASE_DEBUG=line-tables-only \
  cargo build --release --target-dir "$PROJ/.toolchain/prof-target" 2>&1 | grep -E '^(warning|error)' || true)
PBIN="$PROJ/.toolchain/prof-target/x86_64-unknown-linux-musl/release/sessions"
OBJDUMP=$(find "$RUSTUP_HOME/toolchains" -name llvm-objdump | head -1)
out="/tmp/ss-icount-$$"; rm -rf "$out"; mkdir -p "$out"
ev=$(cat "$PROJ/data/$name"/events/*.csv | wc -l); cl=$(cat "$PROJ/data/$name"/clicks/*.csv | wc -l)
ln=$(cat "$PROJ/data/$name"/links/*.csv | wc -l)
s=$(date +%s)
env NCPU=1 ${PROF_ENV:-} "$PROJ/bin/icount" "$out/hist.txt" "$PBIN" "$PROJ/data/$name" "$out/o"
"$OBJDUMP" -d -l --no-show-raw-insn "$PBIN" > "$out/disasm.txt"
python3 -I "$PROJ/tools/icount_report.py" "$out/hist.txt" "$out/disasm.txt" "$PROJ/fast/src" "$ev" "$cl" "$ln" \
  > "$PROJ/bench/icount-$label.txt"
cp "$out/hist.txt" "$PROJ/bench/icount-$label.hist"
rm -rf "$out"
echo "profile written to bench/icount-$label.txt ($(( $(date +%s) - s )) s)"
head -60 "$PROJ/bench/icount-$label.txt"
