#!/usr/bin/env bash
# Runs ON the remote machine: profile a run (lineprof.sh, unless REUSE=1) and
# show the disassembly of one function with the share of samples on every
# instruction (hotasm.sh), found by its symbol name.
# Usage: hotfn.sh SYMBOL_REGEX [DATASET] [MIN_PERCENT_TO_MARK] [MAX_LINES]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
sym="$1"; name="${2:-m20}"; mark="${3:-0.05}"; max="${4:-400}"
PBIN="$PROJ/out/prof-target/x86_64-unknown-linux-musl/release/sessions"
if [ "${REUSE:-0}" != 1 ]; then bash scripts/remote/hotip.sh "$name" nomatchxyz 1 > /dev/null 2>&1; fi
read -r addr size <<< "$(nm -S -C "$PBIN" | grep -E "$sym" | head -1 | cut -d' ' -f1,2)"
[ -n "$addr" ] || { echo "no symbol matching $sym"; exit 1; }
start=$(printf '%x' $((16#$addr))); stop=$(printf '%x' $((16#$addr + 16#$size)))
echo "== $(nm -S -C "$PBIN" | grep -E "$sym" | head -1 | cut -c1-120)"
bash scripts/remote/hotasm.sh "$start" "$stop" "$mark" | head -"$max"
bash scripts/remote/hotasm.sh "$start" "$stop" "$mark" | tail -1
