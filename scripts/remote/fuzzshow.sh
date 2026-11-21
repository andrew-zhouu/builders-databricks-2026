#!/usr/bin/env bash
# Runs ON the remote machine: show what differs for a failing fuzz seed (a
# directory kept by fuzz.sh), in a given mode. Usage: fuzzshow.sh SEED [MODE]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
seed="$1"; mode="${2:-fast}"
d=$(ls -d /tmp/ss-fuzz-*/"$seed" 2>/dev/null | tail -1)
if [ -z "$d" ]; then
  d=/tmp/ss-fuzzshow/$seed; rm -rf "$d"; mkdir -p "$d"
  "$PROJ/.venv/bin/python" -I "$PROJ/tools/fuzz.py" "$seed" "$d/data" "${ROWS:-3000}"
  "$PROJ/.venv/bin/python" -I "$PROJ/reference/reference.py" "$d/data" "$d/expected"
  mkdir -p "$d/$mode"
  NCPU=3 SS_TIMING=1 "$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions" "$d/data" "$d/$mode" 2>"$d/$mode.log"
  "$PROJ/.venv/bin/python" -I "$PROJ/tools/cmp.py" "$d/expected" "$d/$mode" >"$d/$mode.cmp" 2>&1
fi
echo "dir: $d"; cat "$d/data/manifest.json"; echo
echo "--- $mode.cmp"; head -40 "$d/$mode.cmp"
echo "--- $mode.log"; tail -4 "$d/$mode.log"
