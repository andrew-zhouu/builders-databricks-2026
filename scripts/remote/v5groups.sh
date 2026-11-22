#!/usr/bin/env bash
# Runs ON the remote machine: the sets aimed at a decoder that takes eight
# lines at a time (tools/mutate.py `grouping`: batches of nothing but long
# IDs, the long ID always in the same lane), alone and after the long-ID and
# window mutations, on one binary.
# Usage: v5groups.sh BINARY_NAME [FIRST] [COUNT_EACH]
#   env: SETTINGS (default v5), MUTATE_SKIP (default none here), GATE_OK
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
export MUTATE_SKIP="${MUTATE_SKIP:-none}"
FOCUS="grouping longusers,longevents,longclicks,grouping windows,grouping grouping,stretches,purchases longusers,idbytes,grouping,endings" \
  bash scripts/remote/v5battery.sh "$1" focus "${2:-9500}" "${3:-3}"
