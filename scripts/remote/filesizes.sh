#!/usr/bin/env bash
# Runs ON the remote machine: how many event files each generated set has and
# how large the largest is (the start-up calibration needs one of 4 MB).
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
for d in "${@:-dev m5 m20 m60}"; do
  for name in $d; do
    n=$(ls "data/$name/events" | wc -l)
    big=$(ls -S -l "data/$name/events" | sed -n 2p | awk '{print $5}')
    echo "$name: $n event files, largest $big bytes"
  done
done
