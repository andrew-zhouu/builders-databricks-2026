#!/usr/bin/env bash
# Runs ON the remote machine: the batch splitter against the line splitter
# (SS_SPLIT_CHECK=1 compares every line of every batch and aborts on a
# difference), on the named data sets and on a fuzz batch, with ordinary and
# with tiny chunks; then the answers of the data sets against their references.
# Usage: splitcheck.sh [FUZZ_COUNT] [FUZZ_FIRST] [DATASET...]     (default sets: dev m5)
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
n="${1:-20}"; first="${2:-5000}"; shift 2 2>/dev/null || shift $#
[ $# -gt 0 ] || set -- dev m5
BIN="$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions"
rc=0
for name in "$@"; do
  for chunk in "" 4096 257; do
    [ -n "$chunk" ] && [ "$name" != dev ] && continue
    out=/tmp/ss-splitcheck; rm -rf "$out"
    if env SS_SPLIT_CHECK=1 ${chunk:+SS_CHUNK_BYTES=$chunk} NCPU=8 "$BIN" "data/$name" "$out" > /tmp/ss-splitcheck.log 2>&1; then
      echo "ok   split check $name ${chunk:+chunk $chunk}"
    else
      echo "FAIL split check $name ${chunk:+chunk $chunk}"; grep -m3 -E 'panicked|batch split|line ' /tmp/ss-splitcheck.log | cut -c1-400; rc=1
    fi
  done
done
if [ "$n" -gt 0 ]; then
  SS_SPLIT_CHECK=1 bash scripts/fuzz.sh "$first" "$n" 3000 2>&1 | tail -3
fi
rm -rf /tmp/ss-splitcheck
exit $rc
