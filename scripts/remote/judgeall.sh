#!/usr/bin/env bash
# Runs ON the remote machine: one data set through a binary under every
# setting of a mode of idxfuzz.sh's kind that matters for a candidate with the
# wide parser on by default, each compared with the Python reference.
# Usage: judgeall.sh DATA_DIR BINARY_NAME
#   env: YARD=name in bin/ (default dev9): its --slow is compared as well
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
data="$1"; bin="$PROJ/bin/$2"
yard="$PROJ/bin/${YARD:-dev9}"
PY="$PROJ/.venv/bin/python"
W="$(mktemp -d "$PROJ/out/judgeall.XXXXXX")"
"$PY" -I reference/reference.py "$data" "$W/ref" 2> "$W/ref.log" || { echo "the reference fails: $(tail -1 "$W/ref.log")"; exit 1; }
runs=0; fails=0
one() { # label, then the command
  local label="$1"; shift
  rm -rf "$W/out"; mkdir -p "$W/out"; runs=$((runs + 1))
  if ! "$@" "$data" "$W/out" 2> "$W/log"; then
    echo "FAIL $label: exit code; $(tail -2 "$W/log" | tr '\n' ' ' | cut -c1-400)"; fails=$((fails + 1)); return
  fi
  if ! "$PY" -I tools/cmp.py "$W/ref" "$W/out" > "$W/cmp.txt" 2>&1; then
    echo "FAIL $label: $(grep -m5 -E 'rows|A:|B:' "$W/cmp.txt" | tr '\n' ' ' | cut -c1-500)"; fails=$((fails + 1))
  fi
}
one "$(basename "$yard") --slow" "$yard" --slow
one "$(basename "$bin") --slow" "$bin" --slow
for vars in "NCPU=3" "NCPU=1" "NCPU=8" "NCPU=4 SS_SIMD=0" "NCPU=1 SS_SIMD=0" "NCPU=4 SS_WSPLIT=0" "NCPU=4 SS_WEMIT=0" \
            "NCPU=4 SS_SIMD_CHECK=1 SS_SPLIT_CHECK=1" "NCPU=1 SS_SIMD_CHECK=1 SS_SPLIT_CHECK=1" "NCPU=3 SS_SIMD_CHECK=1 SS_WSPLIT=0 SS_WEMIT=0" \
            "NCPU=4 SS_CHUNK_BYTES=64" "NCPU=8 SS_SIMD_CHECK=1 SS_SPLIT_CHECK=1 SS_CHUNK_BYTES=97" "NCPU=4 SS_SIMD=0 SS_CHUNK_BYTES=64" \
            "NCPU=2 SS_SCALAR=1" "NCPU=4 SS_PD_BYTES=203" "NCPU=4 SS_SIMD=0 SS_PD_BYTES=203" \
            "NCPU=4 SS_SIMD=2" "NCPU=1 SS_SIMD=2" "NCPU=4 SS_SIMD=2 SS_SIMD_CHECK=1 SS_SPLIT_CHECK=1" "NCPU=4 SS_SIMD=2 SS_WSPLIT=0 SS_WEMIT=0" \
            "NCPU=8 SS_SIMD=2 SS_SIMD_CHECK=1 SS_SPLIT_CHECK=1 SS_CHUNK_BYTES=97"; do
  # shellcheck disable=SC2086
  one "fast ($vars)" env $vars "$bin"
done
echo "judgeall $(basename "$bin") on $data: $runs runs against the Python reference, $fails failures"
rm -rf "$W"
[ "$fails" = 0 ]
