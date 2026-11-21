#!/usr/bin/env bash
# Differential test on freshly generated seeds: fast binary vs the Python
# reference (and the plain implementation). Small sets, so it is light on RAM.
# Usage: seeds.sh EVENTS CLICKS SEED...
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
PY="$PROJ/.venv/bin/python"
events="$1"; clicks="$2"; shift 2
rc=0
for seed in "$@"; do
  name="s${seed}_${events}"
  if [ ! -f "$PROJ/data/$name/manifest.json" ]; then
    generate --seed "$seed" --events "$events" --clicks "$clicks" --output "$PROJ/data/$name" --threads 2
  fi
  if [ ! -f "$PROJ/expected/$name/user_daily.csv" ]; then
    mkdir -p "$PROJ/expected/$name"
    "$PY" -I "$PROJ/reference/reference.py" "$PROJ/data/$name" "$PROJ/expected/$name"
  fi
  for mode in fast slow; do
    out="$PROJ/out/$name.$mode"; rm -rf "$out"; mkdir -p "$out"
    flag=""; [ "$mode" = slow ] && flag="--slow"
    if ! NCPU="${NCPU:-4}" "$BIN" $flag "$PROJ/data/$name" "$out" 2>"$out.log"; then
      echo "FAIL $name $mode: exit"; tail -3 "$out.log"; rc=1; continue
    fi
    if "$PY" -I "$PROJ/tools/cmp.py" "$PROJ/expected/$name" "$out" >"$out.cmp" 2>&1; then
      echo "ok   $name $mode ($(tr -d '\n ' < "$PROJ/data/$name/manifest.json" | cut -c2-60))"
    else
      echo "FAIL $name $mode"; cat "$out.cmp"; rc=1
    fi
  done
done
exit $rc
