#!/usr/bin/env bash
# Show the serial setup timeline (TIMING=1) a few times.
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
name="${1:-dev}"
for i in 1 2 3; do
  out="/tmp/ss-setup-$$"; rm -rf "$out"; mkdir -p "$out"
  NCPU="${2:-8}" TIMING=1 "$BIN" "$PROJ/data/$name" "$out" 2>&1 | grep -E 'setup|links parsed|people resolved' | sed 's/, events.*//' | tr '\n' ' '
  echo
  rm -rf "$out"
done
