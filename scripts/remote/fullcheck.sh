#!/usr/bin/env bash
# Runs ON the remote machine: the thorough correctness pass for the pushed
# sources, short of the Docker gate: build, fixtures, dev answer, the gate's
# static checks, a large fuzz batch (all modes), six seeds against the Python
# reference, and the 5M / 20M / 20M / 60M sets against the plain implementation.
# Usage: fullcheck.sh [FUZZ_COUNT] [FUZZ_FIRST]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
n="${1:-150}"; first="${2:-1000}"
echo "== build + fixtures + dev + fuzz $first..$((first + n - 1))"
bash scripts/remote/try.sh "$n" "$first" 2>&1 | tail -4
echo "== seeds vs reference"
bash scripts/seeds.sh 200000 50000 2 3 4 5 6 7 2>&1 | tail -13
echo "== scale"
bash scripts/remote/scalecheck.sh
echo "== fullcheck done"
