#!/usr/bin/env bash
# Runs ON the remote machine: the current build's fast path against its own
# plain implementation on one data directory, under each given setting.
# Usage: onecheck.sh DATA_DIR [VAR=VALUE...]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
d="$1"; shift
[ $# -gt 0 ] || set -- X=1
BIN="$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions"
w="out/onecheck-$$"; rm -rf "$w"; mkdir -p "$w/slow"
"$BIN" --slow "$d" "$w/slow" 2> "$w/slow.log" || { echo "FAIL --slow on $d"; tail -3 "$w/slow.log"; }
for setting in "$@"; do
  rm -rf "$w/fast"; mkdir -p "$w/fast"
  if env "$setting" "$BIN" "$d" "$w/fast" 2> "$w/fast.log" && .venv/bin/python -I tools/cmp.py "$w/slow" "$w/fast" > "$w/cmp" 2>&1; then
    echo "ok   $d ($setting): $(grep -c identical "$w/cmp") tables identical"
  else
    echo "FAIL $d ($setting)"; tail -2 "$w/fast.log" | cut -c1-200; head -6 "$w/cmp" | cut -c1-200
  fi
done
rm -rf "$w"
