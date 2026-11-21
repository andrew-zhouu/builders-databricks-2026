#!/usr/bin/env bash
# Correctness gate for the fast binary: all official fixtures, then the dev set
# against the published answer. Usage: check.sh [extra dataset names...]
# Extra datasets are compared with expected/<name> (made by the reference).
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
PY="$PROJ/.venv/bin/python"
rc=0
bash "$PROJ/tools/fixtures.sh" "$BIN" || rc=1
run() { # name expected_dir
  local out="$PROJ/out/$1"
  rm -rf "$out"; mkdir -p "$out"
  if ! SS_TIMING=1 "$BIN" "$PROJ/data/$1" "$out" 2>"$out/log.txt"; then
    echo "FAIL $1: exit code"; tail -5 "$out/log.txt"; rc=1; return
  fi
  tail -1 "$out/log.txt"
  if "$PY" -I "$PROJ/tools/cmp.py" "$2" "$out" >"$out/cmp.txt" 2>&1; then
    echo "ok   $1: $(grep -c identical "$out/cmp.txt") tables identical"
  else
    echo "FAIL $1"; cat "$out/cmp.txt"; rc=1
  fi
}
run dev "$PROJ/official/dev-answer/x/dev-answer"
for name in "$@"; do run "$name" "$PROJ/expected/$name"; done
exit $rc
