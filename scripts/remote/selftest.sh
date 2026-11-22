#!/usr/bin/env bash
# Runs ON the remote machine: do the test tools themselves work? tools/mutate.py
# on one small generated set with every mutation (it must finish, find the
# event key, and leave a data set the plain implementation and the Python
# reference agree on), and tools/fuzz.py with and without FUZZ_IDS=dense (the
# default output must not have changed).
# Usage: selftest.sh [BINARY_NAME_IN_bin]
#   env: NAMES="a b,c ..."   these mutations only (default: every one, and a mix)
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
BIN="$PROJ/bin/${1:-c8}"
PY="$PROJ/.venv/bin/python"
W="$PROJ/out/selftest"; rm -rf "$W"; mkdir -p "$W"
rc=0
generate --seed 11 --events 6000 --clicks 2000 --output "$W/base" --threads 2 > /dev/null 2>&1 || { echo "generator failed"; exit 1; }
for name in ${NAMES:-$("$PY" -I tools/mutate.py list) mix} none; do
  s=$(date +%s%N)
  if ! "$PY" -I tools/mutate.py 5 "$W/base" "$W/$name" "$name" > "$W/$name.log" 2>&1; then
    echo "FAIL mutate $name: $(tail -2 "$W/$name.log" | tr '\n' ' ')"; rc=1; continue
  fi
  ms=$((($(date +%s%N) - s) / 1000000))
  note="$(cut -c1-150 "$W/$name.log")"
  if ! "$BIN" --slow "$W/$name" "$W/$name.slow" 2> "$W/$name.slow.log"; then
    echo "note $name: plain implementation exits with an error ($(tail -1 "$W/$name.slow.log" | cut -c1-100)); $note"; continue
  fi
  "$PY" -I reference/reference.py "$W/$name" "$W/$name.ref" 2> "$W/$name.ref.log" || { echo "FAIL $name: reference failed: $(tail -1 "$W/$name.ref.log")"; rc=1; continue; }
  if "$PY" -I tools/cmp.py "$W/$name.ref" "$W/$name.slow" > "$W/$name.cmp" 2>&1; then
    rows=$(grep -o '[0-9]* rows identical' "$W/$name.cmp" | tr '\n' ' ')
    echo "ok   $name (${ms} ms): $rows| $note"
  else
    echo "FAIL $name: plain implementation differs from the reference: $(grep -m4 -E 'rows|A:|B:' "$W/$name.cmp" | tr '\n' ' ' | cut -c1-300)"; rc=1
  fi
done
# `none` must be the input, byte for byte.
if diff -rq "$W/base" "$W/none" > /dev/null; then echo "ok   none is a plain copy"; else echo "FAIL none differs from its input"; rc=1; fi
# fuzz.py with dense IDs: accepted by the reference. (That its default output
# is unchanged is checked against the committed version, locally.)
"$PY" -I tools/fuzz.py 77 "$W/fz-a" 1500 && FUZZ_IDS=dense "$PY" -I tools/fuzz.py 77 "$W/fz-b" 1500 || { echo "FAIL fuzz.py"; rc=1; }
"$PY" -I reference/reference.py "$W/fz-b" "$W/fz-b.ref" 2> /dev/null || { echo "FAIL reference on dense fuzz"; rc=1; }
echo "fuzz ids plain: $(cut -d, -f1 "$W"/fz-a/events/*.csv | sort -u | wc -l) distinct of $(cat "$W"/fz-a/events/*.csv | wc -l) lines; dense: $(cut -d, -f1 "$W"/fz-b/events/*.csv | sort -u | wc -l) distinct of $(cat "$W"/fz-b/events/*.csv | wc -l) lines; sample: $(cut -d, -f1 "$W"/fz-b/events/part-00000.csv | head -12 | tr '\n' ' ')"
# fuzz.py with the long IDs of version 5 and their near misses: the reference
# and the plain implementation agree on it.
if FUZZ_IDS=long "$PY" -I tools/fuzz.py 77 "$W/fz-c" 1500 && "$PY" -I reference/reference.py "$W/fz-c" "$W/fz-c.ref" 2> /dev/null \
   && "$BIN" --slow "$W/fz-c" "$W/fz-c.slow" 2> /dev/null && "$PY" -I tools/cmp.py "$W/fz-c.ref" "$W/fz-c.slow" > "$W/fz-c.cmp" 2>&1; then
  echo "ok   fuzz long: $(grep -o '[0-9]* rows identical' "$W/fz-c.cmp" | tr '\n' ' ')"
else
  echo "FAIL fuzz long: $(grep -m4 -E 'rows|A:|B:' "$W/fz-c.cmp" 2> /dev/null | tr '\n' ' ' | cut -c1-300)"; rc=1
fi
# What IDs the generated set has, and what the long-ID mutations make of it.
for name in base longusers longevents longclicks; do
  [ -d "$W/$name" ] && echo "ids of $name:" && "$PY" -I tools/idstats.py "$W/$name" | cut -c1-230
done
[ $rc = 0 ] && echo SELFTEST-OK
exit $rc
