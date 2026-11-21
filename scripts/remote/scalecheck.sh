#!/usr/bin/env bash
# Runs ON the remote machine: the current build against the reference answers
# of the large sets (expected/NAME.slow, made by the plain implementation).
# Usage: scalecheck.sh [NAME...]     (default: m5 m20 n20 m60)
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
[ $# -gt 0 ] || set -- m5 m20 n20 m60
rc=0
for name in "$@"; do
  out="out/scale-$name"; rm -rf "$out"; mkdir -p "$out"
  s=$(date +%s%N)
  if ! NCPU=8 "$BIN" "data/$name" "$out" 2>"$out.log"; then echo "FAIL $name: exit code"; tail -3 "$out.log"; rc=1; continue; fi
  ms=$(( ($(date +%s%N) - s) / 1000000 ))
  if .venv/bin/python -I tools/cmp.py "expected/$name.slow" "$out" > "$out.cmp" 2>&1; then
    echo "ok   $name ($ms ms): $(grep -c identical "$out.cmp") tables identical"
  else
    echo "FAIL $name"; head -20 "$out.cmp"; rc=1
  fi
  rm -rf "$out"
done
[ $rc = 0 ] && echo SCALE-OK
exit $rc
