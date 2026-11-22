#!/usr/bin/env bash
# Runs ON the remote machine: does a data set with far more long user IDs than
# usual really send a binary down its overflow road (the table of long IDs
# half full: start over with the plain implementation), and is the answer
# still right? Small sets with `longusers` (which now and then adds thousands
# of link-only long IDs, or renames every user), the binary's own account of
# the run (SS_TIMING=1), and the comparison with the yardstick's --slow.
# Usage: overflow.sh BINARY_NAME [FIRST_SEED] [COUNT] [EVENTS]
#   env: YARD=name in bin/ (default dev9)
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
bin="$PROJ/bin/$1"; first="${2:-1}"; count="${3:-6}"; events="${4:-9000}"
yard="$PROJ/bin/${YARD:-dev9}"
PY="$PROJ/.venv/bin/python"
W="$PROJ/out/overflow"; rm -rf "$W"; mkdir -p "$W"
fails=0
for seed in $(seq "$first" $((first + count - 1))); do
  d="$W/$seed"; mkdir -p "$d"
  generate --seed "$seed" --events "$events" --clicks $((events / 3)) --output "$d/base" --threads 2 > /dev/null 2>&1 || { echo "generator failed"; exit 1; }
  "$PY" -I tools/mutate.py "$seed" "$d/base" "$d/data" longusers > "$d/mutate.log" 2>&1 || { echo "FAIL seed $seed: mutate.py: $(tail -1 "$d/mutate.log")"; fails=$((fails + 1)); continue; }
  "$yard" --slow "$d/data" "$d/slow" 2> /dev/null || { echo "FAIL seed $seed: yardstick"; fails=$((fails + 1)); continue; }
  for vars in "NCPU=8" "NCPU=1" "NCPU=3 SS_CHUNK_BYTES=977"; do
    rm -rf "$d/fast"
    # shellcheck disable=SC2086
    if ! env SS_TIMING=1 $vars "$bin" "$d/data" "$d/fast" 2> "$d/fast.log"; then
      echo "FAIL seed $seed ($vars): exit code; $(tail -2 "$d/fast.log" | tr '\n' ' ' | cut -c1-300)"; fails=$((fails + 1)); continue
    fi
    if "$PY" -I tools/cmp.py "$d/slow" "$d/fast" > "$d/cmp.txt" 2>&1; then verdict="same as the yardstick"; else verdict="DIFFERS: $(grep -m4 -E 'rows|A:|B:' "$d/cmp.txt" | tr '\n' ' ' | cut -c1-300)"; fails=$((fails + 1)); fi
    echo "seed $seed ($vars): $(grep -o 'longusers[^;]*' "$d/mutate.log" | cut -c1-110) | $(grep -i -E 'overflow|plain|start over|again|too many|long user' "$d/fast.log" | tr -s ' ' | tr '\n' ' ' | cut -c1-260) | $verdict"
  done
  rm -rf "$d"
done
echo "overflow $(basename "$bin") seeds $first..$((first + count - 1)): $fails failures"
[ "$fails" = 0 ]
