#!/usr/bin/env bash
# Runs ON the remote machine: interleaved A/B of environment-switch variants of
# the current build, then the per-section profile of each.
# Usage: knobs.sh DATASET ROUNDS VARIANT...      (a variant is VAR=VALUE, or X=1 for the default)
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="$1"; rounds="$2"; shift 2
bash scripts/abx.sh "$name" "$rounds" "$@" 2>&1 | cut -c1-75
if [ "${PROF:-1}" = 1 ]; then
  for v in "$@"; do
    echo "== $v"
    env "$v" bash scripts/prof.sh "$name" 8 2>&1 | grep -E 'parse|dedup|sessions |wall' | cut -c1-78
  done
fi
