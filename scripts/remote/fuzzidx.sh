#!/usr/bin/env bash
# Runs ON the remote machine: the adversarial fuzz corpus (tools/fuzz.py) with
# event IDs that are `e` + small base-36 numbers, many of them repeated
# (FUZZ_IDS=dense: unrelated rows that are copies of one event by the rules),
# against the Python reference, in the modes of scripts/fuzz.sh.
#
# Usage: fuzzidx.sh [BINARY] [COUNT] [FIRST_SEED] [ROWS]
#   env: FUZZ_IDS   dense (default) or plain (the corpus's usual IDs)
#        SETTINGS   plain (default): every mode once
#                   simd: also with SS_SIMD=0 and SS_SIMD_CHECK=1
#                   index: the switches of the index back end (branch exp-index):
#                   SS_INDEX_IDENTITY=1 makes the index the number an ID spells, so
#                   these hand-made IDs land in the index path
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
BIN="${1:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
[ -x "$BIN" ] || BIN="$PROJ/bin/$1"
[ -x "$BIN" ] || { echo "no binary $1"; exit 2; }
count="${2:-10}"; first="${3:-1}"; rows="${4:-3000}"
ids="${FUZZ_IDS:-dense}"
mode="${SETTINGS:-plain}"
PY="$PROJ/.venv/bin/python"
W="$PROJ/out/fuzzidx"; mkdir -p "$W"
fails=0; runs=0
case "$mode" in
  plain) outer=("auto|") ;;
  simd) outer=("auto|" "nosimd|SS_SIMD=0" "check|SS_SIMD_CHECK=1") ;;
  index) outer=(
    "auto|"
    "off|SS_INDEX=0"
    "id64|SS_INDEX_IDENTITY=1 SS_INDEX_BOUND=64"
    "id1500|SS_INDEX_IDENTITY=1 SS_INDEX_BOUND=1500"
    "id4096|SS_INDEX_IDENTITY=1 SS_INDEX_BOUND=4096"
    "id50m|SS_INDEX_IDENTITY=1 SS_INDEX_BOUND=50000000"
    "idpb3|SS_INDEX_IDENTITY=1 SS_INDEX_BOUND=50000000 SS_INDEX_PB=3"
    "idpb7|SS_INDEX_IDENTITY=1 SS_INDEX_BOUND=6000 SS_INDEX_PB=7"
    "key|SS_INDEX_KEY=123456789a SS_INDEX_BOUND=100000") ;;
  *) echo "unknown SETTINGS=$mode"; exit 2 ;;
esac
for seed in $(seq "$first" $((first + count - 1))); do
  d="$W/$seed"; rm -rf "$d"; mkdir -p "$d"
  if ! FUZZ_IDS="$ids" "$PY" -I tools/fuzz.py "$seed" "$d/data" "$rows" 2> "$d/gen.log"; then
    echo "FAIL seed $seed: fuzz.py failed: $(tail -1 "$d/gen.log")"; fails=$((fails + 1)); continue
  fi
  if ! "$PY" -I reference/reference.py "$d/data" "$d/expected" 2> "$d/ref.log"; then
    echo "FAIL seed $seed: reference failed: $(tail -1 "$d/ref.log")"; fails=$((fails + 1)); continue
  fi
  modes=(
    "fast|NCPU=3"
    "one|NCPU=1"
    "eight|NCPU=8"
    "scalar|NCPU=2 SS_SCALAR=1"
    "tiny|NCPU=4 SS_CHUNK_BYTES=$((seed % 97 + 31))"
    "tiny8|NCPU=8 SS_CHUNK_BYTES=$((seed % 1013 + 300))"
    "manyparts|NCPU=4 SS_PD_BYTES=$((seed % 900 + 100))"
    "onepart|NCPU=5 SS_PD_BYTES=1000000000"
  )
  bad=""
  for i in "${outer[@]}"; do
    ilabel="${i%%|*}"; ivars="${i#*|}"
    for m in "${modes[@]}"; do
      mlabel="${m%%|*}"; mvars="${m#*|}"
      # With several outer settings: each in the first mode, the other modes take turns.
      if [ ${#outer[@]} -gt 3 ] && [ "$mlabel" != fast ] && [ $(((seed + ${#ilabel} + ${#mlabel}) % 3)) != 0 ]; then continue; fi
      o="$d/$ilabel-$mlabel"; mkdir -p "$o"; runs=$((runs + 1))
      # shellcheck disable=SC2086
      env $ivars $mvars timeout 120 "$BIN" "$d/data" "$o" 2> "$o.log"; rc=$?
      if [ $rc != 0 ]; then
        echo "FAIL seed $seed ($ids ids) $ilabel-$mlabel ($ivars $mvars): exit code $rc $(tail -1 "$o.log" | cut -c1-200)"; bad="$bad $ilabel-$mlabel"; continue
      fi
      if ! "$PY" -I tools/cmp.py "$d/expected" "$o" > "$o.cmp" 2>&1; then
        echo "FAIL seed $seed ($ids ids) $ilabel-$mlabel ($ivars $mvars): $(grep -m5 -E 'rows|A:|B:' "$o.cmp" | tr '\n' ' ' | cut -c1-400)"; bad="$bad $ilabel-$mlabel"
      else
        rm -rf "$o" "$o.log" "$o.cmp"
      fi
    done
  done
  if [ -n "$bad" ]; then fails=$((fails + 1)); echo "     kept $d"; else rm -rf "$d"; fi
done
echo "fuzzidx $(basename "$BIN") ($ids ids) [$mode] seeds $first..$((first + count - 1)) rows=$rows: $runs runs, $fails failing sets"
[ "$fails" = 0 ]
