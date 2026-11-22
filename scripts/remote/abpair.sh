#!/usr/bin/env bash
# Runs ON the remote machine: the interleaved A/B of whole runs (abx.sh) for
# the given variants at 20M and at 60M rows; only the result lines.
# Usage: abpair.sh VARIANT...          env: R20 (rounds at 20M, default 5), R60 (default 3)
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
bash scripts/abx.sh m20 "${R20:-5}" "$@" 2>&1 | tail -$(( $# + 1 )) | cut -c1-90
bash scripts/abx.sh m60 "${R60:-3}" "$@" 2>&1 | tail -$(( $# + 1 )) | cut -c1-90
