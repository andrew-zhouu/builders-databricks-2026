#!/usr/bin/env bash
# Runs ON the remote machine: a version 5 candidate with the wide parser on by
# default, on the large sets, under the switches that matter at size, against
# the sets' reference answers (expected/NAME.slow, the plain implementation's).
# Usage: v5scale.sh BINARY_NAME [SET...]      (default sets: m5 m20)
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
BIN="$PROJ/bin/$1"; shift
[ -x "$BIN" ] || { echo "no binary $BIN"; exit 2; }
[ $# -gt 0 ] || set -- m5 m20
settings=(
  "default|NCPU=8"
  "one|NCPU=1"
  "three|NCPU=3"
  "base|NCPU=8 SS_SIMD=0"
  "base1|NCPU=1 SS_SIMD=0"
  "checks|NCPU=8 SS_SPLIT_CHECK=1 SS_SIMD_CHECK=1"
  "nowsplit|NCPU=8 SS_WSPLIT=0"
  "nowemit|NCPU=8 SS_WEMIT=0"
  "checksnowemit|NCPU=5 SS_SIMD_CHECK=1 SS_WEMIT=0 SS_WSPLIT=0"
  "chunk1m|NCPU=8 SS_CHUNK_BYTES=1000003"
  "chunk64k|NCPU=7 SS_CHUNK_BYTES=65537 SS_SPLIT_CHECK=1 SS_SIMD_CHECK=1"
  "chunk64kbase|NCPU=7 SS_SIMD=0 SS_CHUNK_BYTES=65537"
  "scalar|NCPU=8 SS_SCALAR=1"
)
runs=0; fails=0
for name in "$@"; do
  [ -d "data/$name" ] && [ -d "expected/$name.slow" ] || { echo "skip $name: no data or no reference answer"; continue; }
  for s in "${settings[@]}"; do
    label="${s%%|*}"; vars="${s#*|}"
    out="out/v5scale-$name-$label"; rm -rf "$out"; mkdir -p "$out"; runs=$((runs + 1))
    # shellcheck disable=SC2086
    if ! env $vars timeout 900 "$BIN" "data/$name" "$out" 2> "$out.log"; then
      echo "FAIL $name $label ($vars): exit code; $(tail -2 "$out.log" | tr '\n' ' ' | cut -c1-300)"; fails=$((fails + 1)); rm -rf "$out"; continue
    fi
    if .venv/bin/python -I tools/cmp.py "expected/$name.slow" "$out" > "$out.cmp" 2>&1; then
      rm -rf "$out" "$out.log" "$out.cmp"
    else
      echo "FAIL $name $label ($vars): $(grep -m4 -E 'rows|A:|B:' "$out.cmp" | tr '\n' ' ' | cut -c1-300)"; fails=$((fails + 1)); rm -rf "$out"
    fi
  done
done
echo "v5scale $(basename "$BIN") sets $*: $runs runs, $fails failures"
[ "$fails" = 0 ]
