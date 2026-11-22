#!/usr/bin/env bash
# Runs ON the remote machine: the sets aimed at the three-window splitters
# (tools/mutate.py `windows`, alone and with the long-ID and splitter
# mutations), on one binary, default path and SS_SIMD=1 with the batch
# splitters' self-check.
# Usage: v5windows.sh BINARY_NAME [FIRST] [COUNT_EACH]
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
FOCUS="windows windows,longevents,longclicks windows,longusers,splitter,clicksplit,endings windows,shapes,clickshapes,interleave" \
  SPLIT_CHECKS=1 bash scripts/remote/v5battery.sh "$1" focus "${2:-3000}" "${3:-4}"
