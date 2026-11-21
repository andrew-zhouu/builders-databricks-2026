#!/usr/bin/env bash
# Make expected answers for generated datasets.
# Usage: mkexpected.sh (py|slow) NAME...
#   py   = the independent Python reference (small sets only)
#   slow = the binary's plain implementation (--slow), for large sets
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
mode="$1"; shift
for name in "$@"; do
  out="$PROJ/expected/$name"
  if [ "$mode" = slow ]; then out="$PROJ/expected/$name.slow"; fi
  rm -rf "$out"; mkdir -p "$out"
  if [ "$mode" = py ]; then
    /usr/bin/time -f "py reference $name: %es %MKB" "$PROJ/.venv/bin/python" -I "$PROJ/reference/reference.py" "$PROJ/data/$name" "$out"
  else
    /usr/bin/time -f "slow $name: %es %MKB" "$BIN" --slow "$PROJ/data/$name" "$out"
  fi
done
