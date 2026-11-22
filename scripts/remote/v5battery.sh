#!/usr/bin/env bash
# Runs ON the remote machine: the battery for version 5 of the generator
# (opaque IDs, one in eight 33 bytes long) on one binary, in lock-aware steps
# (gsteps.sh). The yardstick of the mutated sets is the plain implementation of
# YARD (a binary this tree has checked against the official version 5 answer),
# itself compared with the Python reference on the small sets and on every
# second larger one; the fuzz corpus is compared with the Python reference.
#
# Usage: v5battery.sh BINARY_NAME STAGE [FIRST] [COUNT]
#   STAGE  tools   the long-ID mutations and the fuzz generator's long mode: do the
#                  reference and the plain implementation agree on what they make?
#          mut     COUNT mutated generator sets from seed FIRST (SETTINGS=v5)
#          long    COUNT seeds of the fuzz corpus with long IDs and near misses, from FIRST
#          fuzz    the same corpus with its usual IDs and with dense ones
#          focus   COUNT sets from FIRST for each of the version 5 mutations (or of FOCUS="a b,c ...")
#   env: YARD=name (default dev9), SETTINGS (default v5)
#        MUTATE_SKIP   what tools/mutate.py leaves out (default nultail: event IDs of at
#                      most 9 bytes that end in a NUL, a known defect; give MUTATE_SKIP=none
#                      to have them)
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
export MUTATE_SKIP="${MUTATE_SKIP:-nultail}"
bin="$1"; stage="$2"; first="${3:-1}"; count="${4:-8}"
yard="${YARD:-dev9}"; mode="${SETTINGS:-v5}"
case "$stage" in
  tools)
    NAMES="${TOOLS:-longusers longevents longclicks windows grouping longlinks longlinks,longusers,grouping,endings longusers,longevents,longclicks windows,longusers,longevents,longclicks splitter,longevents clicksplit,longclicks}" \
      bash scripts/remote/gated.sh scripts/remote/selftest.sh "$yard" 2>&1 | cut -c1-300 ;;
  mut)
    SLOW_BIN="$yard" REF_EVERY=2 bash scripts/remote/gsteps.sh mut "$bin" "$mode" "$first" "$count" 3 ;;
  long)
    bash scripts/remote/gsteps.sh long "$bin" "$mode" "$first" "$count" 3 ;;
  sweep)
    # Timestamps and prices where a millisecond or a cent shows (tools/tssweep.py), 4,000 users a set.
    bash scripts/remote/gsteps.sh tssweep "$bin" "$mode" "$first" "$count" 3 "${ROWS:-4000}" ;;
  fuzz)
    bash scripts/remote/gsteps.sh plain "$bin" "$mode" "$first" "$count" 3
    bash scripts/remote/gsteps.sh dense "$bin" "$mode" "$first" "$count" 3 ;;
  focus)
    at="$first"
    for mutation in ${FOCUS:-windows longclicks longusers longevents windows,longevents,longclicks windows,longusers,splitter,clicksplit,endings}; do
      MUTATION="$mutation" SLOW_BIN="$yard" REF_EVERY=2 bash scripts/remote/gsteps.sh mut "$bin" "$mode" "$at" "$count" 3
      at=$((at + count))
    done ;;
  *) echo "unknown stage $stage"; exit 2 ;;
esac
