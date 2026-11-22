#!/usr/bin/env bash
# Runs ON the remote machine: the hot instructions of one function (hotfn.sh:
# a profiled run, unless REUSE=1) with a few instructions around each, instead
# of the whole disassembly. For functions of thousands of instructions.
# Usage: hotspots.sh SYMBOL_REGEX [DATASET] [MIN_PERCENT] [CONTEXT_LINES] [MAX_LINES]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
sym="$1"; name="${2:-m20}"; mark="${3:-0.07}"; ctx="${4:-2}"; max="${5:-160}"
out="/tmp/ss-hotspots-$$.txt"
bash scripts/remote/hotfn.sh "$sym" "$name" "$mark" 100000 > "$out" 2>&1
head -1 "$out" | cut -c1-140
echo "instructions: $(grep -c ':' "$out"), marked: $(grep -c -F ' * ' "$out")"
grep -B"$ctx" -A"$ctx" -F ' * ' "$out" | cut -c1-112 | head -"$max"
tail -1 "$out"
rm -f "$out"
