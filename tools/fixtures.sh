#!/usr/bin/env bash
# Run a pipeline command on every official fixture and diff with tools/cmp.py.
# Usage: fixtures.sh CMD...     (CMD gets DATA_DIR OUT_DIR appended, and as env vars)
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
FIX="$PROJ/official/starter-duckdb/x/starter-duckdb/fixtures"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
pass=0; fail=0; failed=""
for case in $(ls "$FIX" | grep -v README | sort -V); do
  out="$TMP/$case"; mkdir -p "$out"
  if ! DATA_DIR="$FIX/$case/input" OUTPUT_DIR="$out" SCRATCH_DIR="$TMP" "$@" "$FIX/$case/input" "$out" >"$TMP/$case.log" 2>&1; then
    echo "FAIL $case: exit code"; tail -5 "$TMP/$case.log"; fail=$((fail+1)); failed="$failed $case"; continue
  fi
  if "$PROJ/.venv/bin/python" -I "$PROJ/tools/cmp.py" "$FIX/$case/expected" "$out" >"$TMP/$case.cmp" 2>&1; then
    pass=$((pass+1))
  else
    echo "FAIL $case"; cat "$TMP/$case.cmp"; fail=$((fail+1)); failed="$failed $case"
  fi
done
echo "fixtures: $pass passed, $fail failed$failed"
[ "$fail" = 0 ]
