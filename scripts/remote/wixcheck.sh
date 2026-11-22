#!/usr/bin/env bash
# Runs ON the remote machine: correctness of the build in this tree with
# indexed event IDs: is the index on for generated data, fixtures and the dev
# answer, a fuzz batch as it comes and with the identity index (so that the
# corpus's IDs take the index road), both wide self-checks, and the named
# large sets against their references.
# Usage: wixcheck.sh [FUZZ_COUNT] [FUZZ_FIRST] [DATASET...]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
n="${1:-20}"; first="${2:-9000}"; shift 2 2>/dev/null || shift $#
BIN="$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions"
out=/tmp/ss-wix-$$; rm -rf "$out"
NCPU=8 SS_TIMING=1 "$BIN" data/dev "$out" 2>&1 | grep -E 'indexed|done' | cut -c1-150
rm -rf "$out"
bash scripts/check.sh 2>&1 | tail -3
if [ "$n" -gt 0 ]; then
  bash scripts/fuzz.sh "$first" "$n" 3000 2>&1 | tail -2
  for bound in 64 1500 50000000; do
    echo "identity index, bound $bound: $(SS_INDEX_IDENTITY=1 SS_INDEX_BOUND=$bound SS_INDEX_PB=6 bash scripts/fuzz.sh "$((first + 500))" "$n" 3000 2>&1 | tail -1)"
  done
  echo "self-checks on: $(SS_SIMD_CHECK=1 SS_SPLIT_CHECK=1 bash scripts/fuzz.sh "$((first + 900))" "$n" 3000 2>&1 | tail -1)"
fi
for name in "$@"; do
  for setting in "X=1" "SS_SIMD_CHECK=1" "SS_INDEX=0" "SS_INDEX_PB=9"; do
    o="out/wix-$name"; rm -rf "$o"; mkdir -p "$o"
    if env "$setting" NCPU=8 "$BIN" "data/$name" "$o" 2> "$o.log" && .venv/bin/python -I tools/cmp.py "expected/$name.slow" "$o" > "$o.cmp" 2>&1; then
      echo "ok   $name ($setting): $(grep -c identical "$o.cmp") tables identical"
    else
      echo "FAIL $name ($setting)"; tail -3 "$o.log" | cut -c1-300; head -8 "$o.cmp"
    fi
    rm -rf "$o"
  done
done
