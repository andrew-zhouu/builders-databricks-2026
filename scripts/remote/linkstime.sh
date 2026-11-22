#!/usr/bin/env bash
# Runs ON the remote machine: what the links cost in kept binaries: the wall
# time at which the links are parsed, the CPU time of the links section and the
# end of the run, from the section profile (SS_TIMING=1), several runs each,
# interleaved. Settings for all of them come from the environment (SS_SIMD=0).
# Usage: linkstime.sh DATASET RUNS NAME...
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
ds="$1"; runs="$2"; shift 2
cat data/"$ds"/*/*.csv > /dev/null
for r in $(seq "$runs"); do
  for b in "$@"; do
    o="out/linkstime-$$"; rm -rf "$o"
    NCPU=8 SS_TIMING=1 "bin/$b" "data/$ds" "$o" 2>&1 | awk -v b="$b" '
      /\] links parsed/ { w = $2 }
      $1 == "links" { c = $2; rows = $6 }
      /\] done;/ { t = $2 }
      END { printf "%-5s links parsed at %6.2f ms; links section %7.2f ms cpu, %d rows; done at %7.2f ms\n", b, w, c, rows, t }'
    rm -rf "$o"
  done
done
