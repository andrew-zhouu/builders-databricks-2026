#!/usr/bin/env bash
# Make (or move) the local `signal` branch: the given revision plus one line in
# the Dockerfile, ENV SS_SIGNAL=1. An image built from it gives the right
# answer and then sleeps a whole number of seconds that says what the start-up
# calibration found on the machine (see `signal` in fast/src/main.rs): its
# score is that many seconds plus the ordinary time. It is a probe, never a
# candidate: it can only score worse than the same commit without the line.
# Usage: mksignal.sh REV [VALUE]     (VALUE 3: report the four-lane instance)
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
cd "$PROJ"
rev=$(git rev-parse "$1")
value="${2:-1}"
back=$(git rev-parse --abbrev-ref HEAD)
[ -z "$(git status --porcelain)" ] || { echo "working tree not clean"; exit 1; }
git checkout -q -B signal "$rev"
grep -q 'SS_SIGNAL' Dockerfile || sed -i 's|^CMD \["/sessions"\]|# Probe image: the run sleeps 1 to 13 seconds at its end to report what the calibration found. Never submit this as a candidate.\nENV SS_SIGNAL='"$value"'\nCMD ["/sessions"]|' Dockerfile
grep -n 'SS_SIGNAL' Dockerfile
git add Dockerfile
git commit -q -m "Probe image: SS_SIGNAL=1 (the run's duration reports the calibration)"
git checkout -q "$back"
git log --oneline -2 signal
