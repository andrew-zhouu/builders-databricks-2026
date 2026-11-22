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
# While the wide parser is only taken when asked for (parse.rs, WIDE_DEFAULT),
# a default run and a run with it off are the same thing.
scales=4; grep -q 'const WIDE_DEFAULT: bool = false' fast/src/parse.rs && scales=1
{
  echo "== build + fixtures + dev + fuzz $first..$((first + n - 1))"
  bash scripts/remote/try.sh "$n" "$first" 2>&1 | grep -v -e '^warning' -e '^ *|' -e '^ *-->' -e '^ *= note' -e '^$' | tail -6
  echo "== seeds vs reference"
  bash scripts/seeds.sh 200000 50000 2 3 4 5 6 7 2>&1 | tail -13
  echo "== scale"
  bash scripts/remote/scalecheck.sh
  echo "== unit tests"
  (cd fast && cargo test --release 2>&1 | grep -E '^test result|FAILED|panicked' | cut -c1-160)
  if [ "$scales" != 1 ]; then
    echo "== wide parser off: fuzz, seeds, scale"
    SS_SIMD=0 bash scripts/fuzz.sh "$((first + 1000))" 40 3000 2>&1 | tail -2
    SS_SIMD=0 bash scripts/seeds.sh 200000 50000 2 5 2>&1 | tail -4
    SS_SIMD=0 bash scripts/remote/scalecheck.sh m5 m20 m60
    echo "== wide parser self-checks"
    [ -f scripts/remote/splitcheck.sh ] && bash scripts/remote/splitcheck.sh 30 "$((first + 2000))" dev m5 m20 n20
    [ -f scripts/remote/widecheck.sh ] && FUZZ=30 FIRST="$((first + 3000))" bash scripts/remote/widecheck.sh dev m5 m20 n20
    echo "== the wide parser for processors without VBMI2 (SS_SIMD=2): fuzz, seeds, scale, self-checks"
    SS_SIMD=2 bash scripts/fuzz.sh "$((first + 4000))" 40 3000 2>&1 | tail -2
    SS_SIMD=2 bash scripts/seeds.sh 200000 50000 2 5 2>&1 | tail -4
    SS_SIMD=2 bash scripts/remote/scalecheck.sh m5 m20 m60
    SS_SIMD=2 bash scripts/remote/splitcheck.sh 30 "$((first + 5000))" dev m5 m20 n20
    SS_SIMD=2 FUZZ=30 FIRST="$((first + 6000))" bash scripts/remote/widecheck.sh dev m5 m20 n20
    if [ -f scripts/remote/mulcount.sh ]; then
      bash scripts/remote/mulcount.sh fast/target/x86_64-unknown-linux-musl/release/sessions > /tmp/ss-mulcount.$$ 2>&1 \
        && echo "ok   no 512-bit multiplication in the instance without VBMI2" \
        || { echo "FAIL: a 512-bit multiplication in the instance without VBMI2"; tail -12 /tmp/ss-mulcount.$$ | cut -c1-160; }
      rm -f /tmp/ss-mulcount.$$
    fi
    echo "== the four-lane instance (SS_SIMD=3): fuzz, seeds, scale, self-checks"
    SS_SIMD=3 bash scripts/fuzz.sh "$((first + 7000))" 40 3000 2>&1 | tail -2
    SS_SIMD=3 bash scripts/seeds.sh 200000 50000 2 5 2>&1 | tail -4
    SS_SIMD=3 bash scripts/remote/scalecheck.sh m5 m20 m60
    SS_SIMD=3 bash scripts/remote/splitcheck.sh 30 "$((first + 8000))" dev m5 m20
    SS_SIMD=3 FUZZ=30 FIRST="$((first + 9000))" bash scripts/remote/widecheck.sh dev m5 m20
    if [ -f scripts/remote/quadcheck.sh ]; then
      bash scripts/remote/quadcheck.sh fast/target/x86_64-unknown-linux-musl/release/sessions > /tmp/ss-quadcheck.$$ 2>&1 \
        && echo "ok   $(tail -1 /tmp/ss-quadcheck.$$)" \
        || { echo "FAIL: the four-lane instance holds a 512-bit register or a 256-bit multiplication"; tail -12 /tmp/ss-quadcheck.$$ | cut -c1-160; }
      rm -f /tmp/ss-quadcheck.$$
    fi
    echo "== calibration off (the choice by the processor's flags)"
    SS_CALIBRATE=0 bash scripts/remote/scalecheck.sh m5 m20 | grep -v SCALE-OK
  else
    echo "== (the wide parser is off unless asked for: its sections are skipped)"
  fi
  echo "== fullcheck done"
} 2>&1 | tee "$log"
if grep -qE 'FAIL|: [1-9][0-9]* failing|, [1-9][0-9]* failed|panicked|^error' "$log" || ! grep -q 'fullcheck done' "$log"; then
  echo "FULLCHECK-FAILED"; rm -f "$log"; exit 1
fi
[ "$(grep -c SCALE-OK "$log")" = "$scales" ] || { echo "FULLCHECK-FAILED (scale)"; rm -f "$log"; exit 1; }
rm -f "$log"
echo FULLCHECK-OK
