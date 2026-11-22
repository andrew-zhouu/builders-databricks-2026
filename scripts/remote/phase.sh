#!/usr/bin/env bash
# Runs ON the remote machine: the phase times and section profile of one run
# of the current build, for each given environment setting.
# Usage: phase.sh DATASET [VAR=VALUE...]      env: ONLY=regex (lines to show)
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="$1"; shift
[ $# -gt 0 ] || set -- X=1
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
for setting in "$@"; do
  echo "== $setting"
  o="out/phase-$$"; rm -rf "$o"
  env "$setting" NCPU=8 SS_TIMING=1 "$BIN" "data/$name" "$o" 2>&1 | grep -E "${ONLY:-ms\]|^ +[a-z]}" | cut -c1-118
  rm -rf "$o"
done
