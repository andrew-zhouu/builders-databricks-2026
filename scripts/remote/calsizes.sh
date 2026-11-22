#!/usr/bin/env bash
# Runs ON the remote machine: calrace.sh on sets whose one events file is
# written at an exact size around the 4 MiB (4,194,304 bytes) from which a
# pipeline tries its parser instances: a byte under, exactly at, a block over;
# and two heads the other lists do not force (NUL-tailed and odd IDs, the
# longest lines). Each set is a step under the machine's lock (gated.sh).
# Usage: calsizes.sh BINARY_NAME [FIRST_SEED] [REPEATS]
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
bin="$1"; seed="${2:-60}"; repeats="${3:-2}"
wrong=0
# front style, events, what comes before `onefile,front`, the one file's exact size
for plan in "linked 100000 longusers 4194303" "linked 100000 longusers 4194304" "irregular 100000 longusers 4198400" \
            "whale 100000 linkstorm 4194305" "ids 100000 idbytes,irregular 6000000" "longest 100000 windows,shapes 8000000"; do
  set -- $plan
  MUTATE_FILE_BYTES="$4" bash scripts/remote/gated.sh scripts/remote/calrace.sh "$bin" "$seed" "$2" "$repeats" "$1" "$3" | cut -c1-700 || wrong=$((wrong + 1))
  seed=$((seed + 1))
done
echo "calsizes $bin: $wrong sets with a wrong run"
