#!/usr/bin/env bash
# Runs ON the remote machine: are our two yardsticks right for the current
# official materials? The Python reference and the plain implementation
# (BINARY --slow) on the official fixtures, on data/dev against the published
# dev answer, and against each other on freshly generated seeds (kept under
# this tree's out/, nothing is written to the shared data or expected folders).
# Usage: v5check.sh BINARY_NAME [EVENTS] [SEED...]      (default 200000, seeds 11 12 13)
#   env: STAGE=fixtures|dev|seeds to run one part only
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
BIN="$PROJ/bin/$1"; shift
[ -x "$BIN" ] || { echo "no binary $BIN"; exit 2; }
events="${1:-200000}"; [ $# -gt 0 ] && shift
[ $# -gt 0 ] || set -- 11 12 13
PY="$PROJ/.venv/bin/python"
W="$PROJ/out/v5check"; mkdir -p "$W"
answer="$PROJ/official/dev-answer/x/dev-answer"
stage="${STAGE:-all}"
rc=0
same() { # label expected got
  if "$PY" -I tools/cmp.py "$2" "$3" > "$W/cmp.txt" 2>&1; then
    echo "ok   $1: $(grep -c identical "$W/cmp.txt") tables identical ($(grep -o '[0-9]* rows' "$W/cmp.txt" | tr '\n' ' '))"
  else
    echo "FAIL $1: $(grep -m6 -E 'rows|A:|B:|only' "$W/cmp.txt" | tr '\n' ' ' | cut -c1-600)"; rc=1
  fi
}
if [ "$stage" = all ] || [ "$stage" = fixtures ]; then
  echo "fixtures, reference: $(bash tools/fixtures.sh "$PY" -I reference/reference.py 2>&1 | tail -1)"
  echo "fixtures, plain: $(bash tools/fixtures.sh "$BIN" --slow 2>&1 | tail -1)"
  echo "fixtures, fast: $(bash tools/fixtures.sh "$BIN" 2>&1 | tail -1)"
fi
if [ "$stage" = all ] || [ "$stage" = dev ]; then
  echo "dev: $(tr -d '\n ' < data/dev/manifest.json | cut -c1-90), $(cat data/dev/events/*.csv | wc -l) event lines, answer $(wc -l < "$answer/user_daily.csv") + $(wc -l < "$answer/campaign_daily.csv") lines"
  rm -rf "$W/dev-ref" "$W/dev-slow" "$W/dev-fast"; mkdir -p "$W/dev-ref" "$W/dev-slow" "$W/dev-fast"
  if "$PY" -I reference/reference.py data/dev "$W/dev-ref" 2> "$W/dev-ref.log"; then same "reference on dev vs the published answer" "$answer" "$W/dev-ref"
  else echo "FAIL reference on dev: $(tail -2 "$W/dev-ref.log" | tr '\n' ' ')"; rc=1; fi
  if "$BIN" --slow data/dev "$W/dev-slow" 2> "$W/dev-slow.log"; then same "plain on dev vs the published answer" "$answer" "$W/dev-slow"
  else echo "FAIL plain on dev: $(tail -2 "$W/dev-slow.log" | tr '\n' ' ')"; rc=1; fi
  if NCPU=8 "$BIN" data/dev "$W/dev-fast" 2> "$W/dev-fast.log"; then same "fast on dev vs the published answer" "$answer" "$W/dev-fast"
  else echo "FAIL fast on dev: $(tail -2 "$W/dev-fast.log" | tr '\n' ' ')"; rc=1; fi
fi
if [ "$stage" = all ] || [ "$stage" = seeds ]; then
  for seed in "$@"; do
    d="$W/s$seed"; rm -rf "$d"; mkdir -p "$d"
    if ! generate --seed "$seed" --events "$events" --clicks $((events / 4)) --output "$d/data" --threads 2 > "$d/gen.log" 2>&1; then
      echo "FAIL seed $seed: generator: $(tail -1 "$d/gen.log")"; rc=1; continue
    fi
    # How many IDs of each kind have the long form (33 bytes), to see which generator this is.
    shape="$(cat "$d"/data/events/*.csv | "$PY" -I -c '
import sys
n = long_e = long_u = 0
for line in sys.stdin.buffer:
    f = line.rstrip(b"\n").replace(b"\"", b"").split(b",")
    if len(f) == 7 and f[0] != b"event_id":
        n += 1; long_e += len(f[0]) == 33; long_u += len(f[1]) == 33
print(f"{n} lines, {long_e} long event IDs, {long_u} long user IDs")')"
    mkdir -p "$d/ref" "$d/slow" "$d/fast"
    if ! "$PY" -I reference/reference.py "$d/data" "$d/ref" 2> "$d/ref.log"; then echo "FAIL seed $seed: reference: $(tail -1 "$d/ref.log")"; rc=1; continue; fi
    if "$BIN" --slow "$d/data" "$d/slow" 2> "$d/slow.log"; then same "seed $seed ($shape): plain vs reference" "$d/ref" "$d/slow"
    else echo "FAIL seed $seed: plain: $(tail -2 "$d/slow.log" | tr '\n' ' ')"; rc=1; fi
    if NCPU=8 "$BIN" "$d/data" "$d/fast" 2> "$d/fast.log"; then same "seed $seed: fast vs reference" "$d/ref" "$d/fast"
    else echo "FAIL seed $seed: fast: $(tail -2 "$d/fast.log" | tr '\n' ' ')"; rc=1; fi
    [ $rc = 0 ] && rm -rf "$d"
  done
fi
[ $rc = 0 ] && echo V5CHECK-OK
exit $rc
