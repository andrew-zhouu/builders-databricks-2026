#!/usr/bin/env bash
# Runs ON the remote machine: the wide parser against the ordinary one, batch
# by batch (SS_SIMD_CHECK=1 stops at the first line they disagree on), over the
# named data sets and, with FUZZ=N, over N fuzz inputs; then the output of
# each data set against its reference answer with the wide path on.
# Usage: widecheck.sh [DATASET...]        env: FUZZ=count FIRST=seed
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
BIN="$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions"
[ $# -gt 0 ] || set -- dev m5
rc=0
for name in "$@"; do
  out=/tmp/ss-wc-out; rm -rf "$out"; mkdir -p "$out"
  if SS_SIMD_CHECK=1 NCPU=8 "$BIN" "data/$name" "$out" > /tmp/ss-wc.log 2>&1; then
    echo "ok   $name: both paths agree on every batch"
  else
    echo "FAIL $name"; head -12 /tmp/ss-wc.log | cut -c1-300; rc=1
  fi
  rm -rf "$out"
done
if [ "${FUZZ:-0}" -gt 0 ]; then
  SS_SIMD_CHECK=1 bash scripts/fuzz.sh "${FIRST:-7000}" "$FUZZ" 3000 2>&1 | tail -4 || rc=1
fi
exit $rc
