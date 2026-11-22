#!/usr/bin/env bash
# Runs ON the remote machine: the sets aimed at a splitter and a decoder that
# take a few lines a step (four, or eight): a line the splitter must refuse
# after every 1st to 9th line (tools/mutate.py `interleave`), groupings with
# the odd line in every 2nd to 9th place, runs of short and of malformed lines
# (`splitter`, `clicksplit`), fields at the window edges (`windows`).
# Usage: v5lanes.sh BINARY_NAME [FIRST] [COUNT_EACH]
#   env: SETTINGS (default v5), MUTATE_SKIP (default none here), GATE_OK
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
export MUTATE_SKIP="${MUTATE_SKIP:-none}"
FOCUS="interleave interleave,grouping windows,interleave,endings splitter,clicksplit,interleave longusers,longevents,interleave,grouping interleave,interleave,shapes" \
  bash scripts/remote/v5battery.sh "$1" focus "${2:-12000}" "${3:-2}"
