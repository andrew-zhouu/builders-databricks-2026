#!/usr/bin/env bash
# Runs ON the remote machine: shrink a failing set kept by idxfuzz.sh or
# fuzzidx.sh (tools/shrink.py), against the plain implementation of YARD.
# Usage: shrink.sh KEPT_DIR BINARY_NAME [VAR=VALUE ...] [--slow]
#   KEPT_DIR  e.g. out/idxfuzz/1234 or out/fuzzidx/7 (its data/ is shrunk into KEPT_DIR/shrunk)
#   env: YARD=name in bin/ (default dev9)
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
kept="$1"; bin="$2"; shift 2
[ -d "$kept/data" ] || { echo "no $kept/data"; exit 2; }
YARD="$PROJ/bin/${YARD:-dev9}" timeout 1500 "$PROJ/.venv/bin/python" -I tools/shrink.py "$kept/data" "$kept/shrunk" "$PROJ/bin/$bin" "$@" 2>&1 | cut -c1-600 | tail -60
