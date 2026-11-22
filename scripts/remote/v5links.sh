#!/usr/bin/env bash
# Runs ON the remote machine: the sets aimed at a short cut for long link
# lines (tools/mutate.py `longlinks`), alone and with what else touches links
# and users, and the sets aimed at the per-batch search for long users
# (`grouping`, with its 63/64/65-line patterns), on one binary.
# Usage: v5links.sh BINARY_NAME [FIRST] [COUNT_EACH]
#   env: SETTINGS (default v5), MUTATE_SKIP (default none here), GATE_OK
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
export MUTATE_SKIP="${MUTATE_SKIP:-none}"
FOCUS="longlinks longlinks,endings longlinks,longusers longlinks,linkshapes,links,nested grouping longusers,grouping,endings longlinks,longusers,grouping longusers,longclicks,grouping" \
  bash scripts/remote/v5battery.sh "$1" focus "${2:-9700}" "${3:-3}"
