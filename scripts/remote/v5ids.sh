#!/usr/bin/env bash
# Runs ON the remote machine: the sets aimed at how IDs are told apart
# (tools/mutate.py idbytes, with the NUL-tailed short IDs; longusers,
# longevents, longclicks), on one binary.
# Usage: v5ids.sh BINARY_NAME [FIRST] [COUNT_EACH]
#   env: SETTINGS (default v5), MUTATE_SKIP (default none here), GATE_OK
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
export MUTATE_SKIP="${MUTATE_SKIP:-none}"
FOCUS="idbytes longusers longevents longclicks idbytes,copies,longevents" \
  bash scripts/remote/v5battery.sh "$1" focus "${2:-9000}" "${3:-3}"
