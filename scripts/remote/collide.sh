#!/usr/bin/env bash
# Runs ON the remote machine: a tiny set of event IDs made to collide in the
# fast pipeline's hash (tools/collide.py), judged under every setting against
# the Python reference (judgeall.sh). Light.
# Usage: collide.sh BINARY_NAME [PAIRS] [SEED]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
W="$PROJ/out/collide"; rm -rf "$W"; mkdir -p "$W"
"$PROJ/.venv/bin/python" -I tools/collide.py data/dev "$W/data" "${2:-4}" "${3:-1}" | cut -c1-200 || exit 1
bash scripts/remote/judgeall.sh "$W/data" "$1" 2>&1 | cut -c1-700
