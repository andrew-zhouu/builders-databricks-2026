#!/usr/bin/env bash
# Runs ON the remote machine: one large set, the same command many times. For
# a pipeline that tries its parser instances on the head of the largest events
# file WHILE its other threads read the links: a set with one events file of
# many megabytes whose head is full of users that have links, and hundreds of
# thousands of link lines (tools/mutate.py linkstorm, onefile, front with
# MUTATE_FRONT). The answer must be the yardstick's every single time; a
# difference that comes and goes is a race.
#
# Usage: calrace.sh BINARY_NAME [SEED] [EVENTS] [REPEATS] [FRONT] [MUTATIONS_BEFORE]
#   FRONT: the style of `front` (default linked); MUTATIONS_BEFORE: default longusers,linkstorm
#   env: YARD=name in bin/ (default dev9); MUTATE_LINKS=n (link lines, default by seed)
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
bin="$PROJ/bin/$1"; seed="${2:-1}"; events="${3:-200000}"; repeats="${4:-10}"; frontstyle="${5:-linked}"; before="${6:-longusers,linkstorm}"
yard="$PROJ/bin/${YARD:-dev9}"
PY="$PROJ/.venv/bin/python"
W="$PROJ/out/calrace/$seed"; rm -rf "$W"; mkdir -p "$W"
generate --seed "$seed" --events "$events" --clicks $((events / 4)) --output "$W/base" --threads 2 > /dev/null 2>&1 || { echo "generator failed"; exit 1; }
MUTATE_SKIP=none MUTATE_FRONT="$frontstyle" "$PY" -I tools/mutate.py "$seed" "$W/base" "$W/data" "$before,onefile,front" > "$W/mutate.log" 2>&1 \
  || { echo "mutate.py failed: $(tail -1 "$W/mutate.log")"; exit 1; }
rm -rf "$W/base"
echo "seed $seed: $(cut -c1-330 "$W/mutate.log")"
echo "   largest events file $(ls -S "$W/data/events" | head -1 | xargs -I{} stat -c %s "$W/data/events/{}") bytes, $(cat "$W"/data/links/*.csv | wc -l) link lines"
"$yard" --slow "$W/data" "$W/slow" 2> "$W/slow.log" || { echo "the yardstick fails: $(tail -1 "$W/slow.log")"; exit 1; }
if [ "$events" -le 60000 ]; then
  "$PY" -I reference/reference.py "$W/data" "$W/ref" 2> /dev/null && "$PY" -I tools/cmp.py "$W/ref" "$W/slow" > "$W/ref.cmp" 2>&1 \
    && echo "   yardstick == Python reference" || echo "   FAIL yardstick differs from the Python reference: $(grep -m3 -E 'rows|A:|B:' "$W/ref.cmp" | tr '\n' ' ' | cut -c1-300)"
fi
runs=0; wrong=0; wide=0; ordinary=0
for k in $(seq "$repeats"); do
  for vars in "NCPU=8" "NCPU=2" "NCPU=4" "NCPU=1" "NCPU=8 SS_PD_BYTES=300" "NCPU=3"; do
    o="$W/out"; rm -rf "$o"; mkdir -p "$o"; runs=$((runs + 1))
    # shellcheck disable=SC2086
    if ! env SS_TIMING=1 $vars timeout 300 "$bin" "$W/data" "$o" 2> "$o.log"; then
      wrong=$((wrong + 1)); echo "   FAIL run $k ($vars): exit code; $(tail -2 "$o.log" | tr '\n' ' ' | cut -c1-300)"; continue
    fi
    case "$(grep -o 'parser instance [0-9]' "$o.log")" in *" 0") ordinary=$((ordinary + 1)) ;; *" "[1-9]) wide=$((wide + 1)) ;; esac
    if ! "$PY" -I tools/cmp.py "$W/slow" "$o" > "$o.cmp" 2>&1; then
      wrong=$((wrong + 1)); echo "   FAIL run $k ($vars): $(grep -m5 -E 'rows|A:|B:' "$o.cmp" | tr '\n' ' ' | cut -c1-400)"
      [ -d "$W/bad" ] || cp -r "$o" "$W/bad"
    fi
  done
done
echo "calrace $(basename "$bin") seed $seed ($events events, front: $frontstyle): $runs runs, $wrong wrong; the wide instance chosen in $wide, the ordinary one in $ordinary"
[ "$wrong" = 0 ] && rm -rf "$W"
[ "$wrong" = 0 ]
