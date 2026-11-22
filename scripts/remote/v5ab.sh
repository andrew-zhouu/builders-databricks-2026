#!/usr/bin/env bash
# Runs ON the remote machine (under the lock): the pushed sources on the
# version-5 data: quick checks (fixtures, dev answer, a fuzz batch, m5 and m20
# against their references), then wall time at 20M and 60M for the given
# variants (abx.sh's: env settings, SSBIN=name for a kept binary).
# Usage: v5ab.sh ROUNDS_20M ROUNDS_60M VARIANT...
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
r20="$1"; r60="$2"; shift 2
bash scripts/remote/try.sh 12 3100 2>&1 | grep -E 'fixtures:|ok   dev|FAIL|failing|^error' | cut -c1-160
bash scripts/remote/scalecheck.sh m5 m20
echo "load before timing: $(cut -d' ' -f1-3 /proc/loadavg)"
bash scripts/abx.sh m20 "$r20" "$@" 2>&1 | tail -$(( $# + 2 ))
[ "$r60" -gt 0 ] && bash scripts/abx.sh m60 "$r60" "$@" 2>&1 | tail -$(( $# + 2 ))
