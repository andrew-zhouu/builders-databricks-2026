#!/usr/bin/env bash
# Make (or move) the `diag` branch: the given revision plus one line in the
# Dockerfile that turns the diagnostic mode on. An image built from it reports
# the machine and kernel it runs on and several profiled runs to stderr, then
# FAILS ON PURPOSE (exit 1) so that a scorer which only shows logs for failed
# runs shows this one. It can never score. See fast/src/diag.rs.
# Usage: mkdiag.sh [REV]      (default: main)
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
cd "$PROJ"
rev=$(git rev-parse "${1:-main}")
back=$(git rev-parse --abbrev-ref HEAD)
[ -z "$(git status --porcelain)" ] || { echo "working tree not clean"; exit 1; }
git checkout -q -B diag "$rev"
grep -q 'SS_DIAG' Dockerfile || sed -i 's|^CMD \["/sessions"\]|# Diagnostic image: reports the environment and exits 1 on purpose. Never submit this for a score.\nENV SS_DIAG=1\nCMD ["/sessions"]|' Dockerfile
# ... and builds the microbenchmarks in (fast/src/probe.rs).
grep -q 'features probe' Dockerfile || sed -i 's|^RUN cargo build --release --locked |RUN cargo build --release --locked --features probe |' Dockerfile
grep -n 'SS_DIAG\|features probe' Dockerfile
git add Dockerfile
git commit -q -m "Diagnostic image: SS_DIAG=1 (reports the environment, fails on purpose)

Co-Authored-By: Claude Fable 5.2 <noreply@anthropic.com>"
git checkout -q "$back"
git log --oneline -2 diag
