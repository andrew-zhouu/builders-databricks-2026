#!/usr/bin/env bash
# Full local gate before keeping a change: build, no 512-bit code in the binary,
# fixtures + dev answer, generated seeds vs the Python reference, a fuzz batch.
# Usage: gate.sh [FUZZ_COUNT]
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
S="$PROJ/scripts"
bash "$S/build.sh" || exit 1
BIN="$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions"
OBJDUMP=$(find "$RUSTUP_HOME/toolchains" -name llvm-objdump | head -1)
wide=$("$OBJDUMP" -d --no-show-raw-insn "$BIN" | grep -cE '%zmm|%k[0-7]')
echo "512-bit / mask-register instructions in binary: $wide"
[ "$wide" = 0 ] || exit 1
bash "$S/check.sh" || exit 1
bash "$S/seeds.sh" 200000 50000 2 3 4 5 6 7 | grep -v '^ok' ; [ "${PIPESTATUS[0]}" = 0 ] || exit 1
echo "seeds 2-7 ok"
bash "$S/fuzz.sh" "${FUZZ_FIRST:-1}" "${1:-25}" 3000 || exit 1
echo "GATE PASSED"
