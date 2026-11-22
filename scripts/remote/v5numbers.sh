#!/usr/bin/env bash
# Runs ON the remote machine: the sets aimed at how timestamps and prices are
# turned into numbers (tools/mutate.py `numbers`, alone and with the mutations
# that put many such lines into one batch or change what else a line holds),
# on one binary.
# Usage: v5numbers.sh BINARY_NAME [FIRST] [COUNT_EACH]
#   env: SETTINGS (default v5), MUTATE_SKIP (default none here), GATE_OK
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
export MUTATE_SKIP="${MUTATE_SKIP:-none}"
FOCUS="numbers numbers,grouping numbers,shapes,lateness numbers,values,purchases,grouping numbers,longusers,longevents numbers,windows,endings" \
  bash scripts/remote/v5battery.sh "$1" focus "${2:-9900}" "${3:-3}"
