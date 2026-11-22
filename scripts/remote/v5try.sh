#!/usr/bin/env bash
# Runs ON the remote machine (under the lock): the pushed sources on the
# version-5 data: build, fixtures, dev answer, a fuzz batch, the named large
# sets against their references, then wall time against kept binaries.
# Usage: v5try.sh [FUZZ_COUNT] [ROUNDS] [KEPT...]      (default kept: tbl)
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
n="${1:-20}"; rounds="${2:-5}"; shift 2 2>/dev/null || shift $#
[ $# -gt 0 ] || set -- tbl
bash scripts/remote/try.sh "$n" 3000 2>&1 | grep -v -e '^warning' -e '^ *|' -e '^ *-->' -e '^ *= note' -e '^$' -e '^note' -e '^ *[0-9]* |' | tail -5 | cut -c1-200
bash scripts/remote/scalecheck.sh m5 m20
variants="X=1"
for k in "$@"; do variants="$variants SSBIN=$k"; done
bash scripts/abx.sh m20 "$rounds" $variants 2>&1 | tail -$(( $# + 3 ))
NCPU=8 SS_TIMING=1 fast/target/x86_64-unknown-linux-musl/release/sessions data/m20 out/v5-timing 2>&1 | cut -c1-175 | tail -24; rm -rf out/v5-timing
