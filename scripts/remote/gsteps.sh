#!/usr/bin/env bash
# Runs ON the remote machine: a correctness battery in short steps, each of
# which takes the machine's lock like `remote.sh run`, for the minute or two
# it lasts, so that others get their turn in between. Start it with
# `remote.sh quick` (which takes no lock itself; under `remote.sh run` the
# steps would wait for their own caller). Only with GATE_OK=1, and while a
# gate holds the lock for a long time (a tmux session named like w2 or v1
# exists), do the steps run without the lock: an exception the lead grants
# for the length of a gate.
#
# Usage: gsteps.sh KIND BINARY MODE FIRST COUNT [STEP] [ROWS]
#   KIND   mut    idxfuzz.sh (mutated generator sets; env MUTATION, EVENTS pass through)
#          dense  fuzzidx.sh with FUZZ_IDS=dense
#          plain  fuzzidx.sh with FUZZ_IDS=plain
#          long   fuzzidx.sh with FUZZ_IDS=long
#   MODE   the SETTINGS of those scripts (plain, v5, simd, split, wix, index)
#   env SLOW_BIN, REF_EVERY, MUTATION, EVENTS, ONLY pass through to idxfuzz.sh
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
kind="$1"; bin="$2"; mode="$3"; first="$4"; count="$5"; step="${6:-4}"; rows="${7:-3000}"
log="$(mktemp)"
s="$first"; last=$((first + count))
while [ "$s" -lt "$last" ]; do
  n=$((last - s)); [ "$n" -gt "$step" ] && n="$step"
  case "$kind" in
    mut) cmd="SETTINGS=$mode bash scripts/remote/idxfuzz.sh $bin $n $s" ;;
    dense|plain|long) cmd="SETTINGS=$mode FUZZ_IDS=$kind bash scripts/remote/fuzzidx.sh $bin $n $s $rows" ;;
    *) echo "unknown kind $kind"; exit 2 ;;
  esac
  if [ -n "${GATE_OK:-}" ] && tmux ls 2>/dev/null | grep -q -E '^[a-z]+[0-9]+:'; then
    bash -c "$cmd" 2>&1 | tee -a "$log" | grep -v -E '^(idxfuzz|fuzzidx) .* 0 failing sets' | cut -c1-900
  else
    flock /tmp/ss-heavy.lock bash -c "$cmd" 2>&1 | tee -a "$log" | grep -v -E '^(idxfuzz|fuzzidx) .* 0 failing sets' | cut -c1-900
  fi
  s=$((s + n))
done
awk -v what="$kind $bin [$mode] seeds $first..$((last - 1))${MUTATION:+ [$MUTATION]}" '
  /^(idxfuzz|fuzzidx) / { for (i = 1; i <= NF; i++) { if ($i == "runs,") runs += $(i - 1); if ($i == "failing") fails += $(i - 1) }
                           if (match($0, /\(([0-9]+) of ([0-9]+) sets took/)) { split(substr($0, RSTART + 1), a, " "); indexed += a[1]; sets += a[3] } }
  END { printf "TOTAL %s: %d runs, %d failing sets%s\n", what, runs, fails, sets ? sprintf(" (%d of %d sets indexed by default)", indexed, sets) : "" }' "$log"
rm -f "$log"
