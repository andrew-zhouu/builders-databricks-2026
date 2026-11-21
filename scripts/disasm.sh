#!/usr/bin/env bash
# Disassemble one function of the release binary (needs the llvm-tools component).
# Usage: disasm.sh SYMBOL_SUBSTRING [MAX_LINES]
. /home/andre/builders/databricks/scripts/env.sh
BIN="$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions"
OBJDUMP=$(find "$RUSTUP_HOME/toolchains" -name llvm-objdump | head -1)
if [ -z "$OBJDUMP" ]; then
  rustup component add llvm-tools >/dev/null 2>&1
  OBJDUMP=$(find "$RUSTUP_HOME/toolchains" -name llvm-objdump | head -1)
fi
NM=$(dirname "$OBJDUMP")/llvm-nm
sym=$("$NM" -C --defined-only "$BIN" | grep -i "$1" | head -1 | awk '{print $3}')
raw=$("$NM" --defined-only "$BIN" | grep -i "$1" | head -1 | awk '{print $3}')
echo "symbol: $sym"
"$NM" -S --size-sort -C "$BIN" | grep -i "$1" | head -5
"$OBJDUMP" -d --no-show-raw-insn -M intel --disassemble-symbols="$raw" "$BIN" | head -"${2:-400}"
