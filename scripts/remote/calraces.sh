#!/usr/bin/env bash
# Runs ON the remote machine: calrace.sh over the things a trial run on the
# head of the largest events file can meet (one set and many runs for each),
# each as a step under the machine's lock (gated.sh; GATE_OK=1 during a gate).
# Usage: calraces.sh BINARY_NAME [FIRST_SEED] [REPEATS]
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
bin="$1"; seed="${2:-1}"; repeats="${3:-3}"
wrong=0
# front style, events, what comes before `onefile,front`
for plan in "linked 200000 longusers,linkstorm" "whale 150000 linkstorm" "long 120000 longusers,longevents,linkstorm" \
            "irregular 200000 longusers,userlengths,linkstorm" "purchases 100000 numbers,purchases,linkstorm" \
            "junk 100000 linkstorm" "odd 150000 values,lateness,copies,linkstorm" "ids 100000 idbytes,irregular,linkstorm" \
            "longest 100000 windows,shapes,linkstorm" "linked 300000 longlinks,linkstorm" "linked 60000 longusers,linkstorm" \
            "linked 45000 linkstorm" "irregular 100000 longusers" "long 200000 longusers,longusers"; do
  set -- $plan
  bash scripts/remote/gated.sh scripts/remote/calrace.sh "$bin" "$seed" "$2" "$repeats" "$1" "$3" | cut -c1-700 || wrong=$((wrong + 1))
  seed=$((seed + 1))
done
echo "calraces $bin: $wrong sets with a wrong run"
