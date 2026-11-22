#!/usr/bin/env bash
# Runs ON the remote machine: the checks for one step of the wide parser, on
# the sources as pushed. Build, fixtures, dev answer and a fuzz batch end to
# end (the binary is kept as bin/NAME); the wide decode against the ordinary
# one and the batch splitter against the line splitter, batch by batch, over
# the data sets and more fuzz inputs; the large sets against their reference
# answers. Optionally an interleaved A/B at the end.
# Usage: stepcheck.sh NAME [FUZZ_FIRST] [AB_DATASET AB_ROUNDS VARIANT...]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
set -o pipefail
name="$1"; first="${2:-7600}"; shift 2 2>/dev/null || shift $#
rc=0
bash scripts/remote/try.sh 40 "$first" 2>&1 | tail -5 | cut -c1-110 || rc=1
cp fast/target/x86_64-unknown-linux-musl/release/sessions "bin/$name" && echo "kept bin/$name"
FUZZ=40 FIRST=$((first + 40)) bash scripts/remote/widecheck.sh dev m5 m20 n20 || rc=1
bash scripts/remote/splitcheck.sh 20 $((first + 80)) dev m5 2>&1 | tail -8 || rc=1
bash scripts/remote/scalecheck.sh m5 m20 n20 m60 || rc=1
echo "STEP $name rc=$rc"
if [ $# -ge 3 ]; then
  ds="$1"; rounds="$2"; shift 2
  bash scripts/abx.sh "$ds" "$rounds" "$@" 2>&1 | tail -$(( $# + 1 )) | cut -c1-90
fi
exit $rc
