#!/usr/bin/env bash
# Runs ON the remote machine: the thorough correctness pass for the pushed
# sources, short of the Docker gate: build, fixtures, dev answer, a large fuzz
# batch (all modes), six seeds against the Python reference, the 5M / 20M /
# 20M / 60M sets against the plain implementation; then the same parser paths
# apart: with the wide instance off (what a processor without AVX-512 VBMI2
# runs), and the wide instance's two self-checks (its decode and its splitter
# against the ordinary ones, line by line).
# Exits 1 if anything failed.
# Usage: fullcheck.sh [FUZZ_COUNT] [FUZZ_FIRST]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
n="${1:-150}"; first="${2:-1000}"
log=/tmp/ss-fullcheck.$$.log
{
  echo "== build + fixtures + dev + fuzz $first..$((first + n - 1))"
  bash scripts/remote/try.sh "$n" "$first" 2>&1 | grep -v -e '^warning' -e '^ *|' -e '^ *-->' -e '^ *= note' -e '^$' | tail -6
  echo "== seeds vs reference"
  bash scripts/seeds.sh 200000 50000 2 3 4 5 6 7 2>&1 | tail -13
  echo "== scale"
  bash scripts/remote/scalecheck.sh
  echo "== wide parser off: fuzz, seeds, scale"
  SS_SIMD=0 bash scripts/fuzz.sh "$((first + 1000))" 40 3000 2>&1 | tail -2
  SS_SIMD=0 bash scripts/seeds.sh 200000 50000 2 5 2>&1 | tail -4
  SS_SIMD=0 bash scripts/remote/scalecheck.sh m5 m20 m60
  echo "== wide parser self-checks"
  [ -f scripts/remote/splitcheck.sh ] && bash scripts/remote/splitcheck.sh 30 "$((first + 2000))" dev m5 m20 n20
  [ -f scripts/remote/widecheck.sh ] && FUZZ=30 FIRST="$((first + 3000))" bash scripts/remote/widecheck.sh dev m5 m20 n20
  echo "== fullcheck done"
} 2>&1 | tee "$log"
if grep -qE 'FAIL|: [1-9][0-9]* failing|, [1-9][0-9]* failed|panicked|^error' "$log" || ! grep -q 'fullcheck done' "$log"; then
  echo "FULLCHECK-FAILED"; rm -f "$log"; exit 1
fi
[ "$(grep -c SCALE-OK "$log")" = 2 ] || { echo "FULLCHECK-FAILED (scale)"; rm -f "$log"; exit 1; }
rm -f "$log"
echo FULLCHECK-OK
