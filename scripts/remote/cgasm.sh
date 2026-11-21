#!/usr/bin/env bash
# Runs ON the remote machine: the hot path of a function as assembly, each
# instruction with how often it ran per event row (valgrind, single thread).
# Shows exactly where the instructions of a loop go, source line by source line.
# Usage: cgasm.sh FUNCTION_REGEX [DATASET] [MIN_PER_ROW]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
fn="${1:-events_chunk}"; name="${2:-dev}"; min="${3:-0.02}"
command -v valgrind >/dev/null || sudo apt-get install -y -qq valgrind >/dev/null 2>&1
(cd fast && CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo build --release --target-dir "$PROJ/.toolchain/prof-target" 2>&1 | grep -E '^error' -A8)
PBIN="$PROJ/.toolchain/prof-target/x86_64-unknown-linux-musl/release/sessions"
out=/tmp/ss-cgasm; rm -rf "$out"; mkdir -p "$out"
rows=$(cat "data/$name"/events/*.csv | wc -l)
NCPU=1 SS_NOHUGE=1 ${RUN_ENV:-} valgrind --tool=callgrind --dump-instr=yes --compress-pos=no --compress-strings=no \
  --callgrind-out-file="$out/cg.out" "$PBIN" "data/$name" "$out/o" > "$out/log" 2>&1 || { tail -5 "$out/log"; exit 1; }
objdump -d -l --no-show-raw-insn -C "$PBIN" > "$out/asm.txt"
python3 -I "$PROJ/tools/cgasm.py" "$out/cg.out" "$out/asm.txt" "$rows" "$fn" "$min"
