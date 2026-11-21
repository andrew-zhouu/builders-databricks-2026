#!/usr/bin/env bash
# Runs ON the remote machine: sampled time by source line (lineprof.sh), kept in
# /tmp/ss-lp.txt, then the lines matching a regex with their share summed.
# Usage: lp.sh [DATASET] [REGEX] [TOP]        env: REUSE=1 (no new run)
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="${1:-m20}"; re="${2:-.}"; top="${3:-50}"
if [ "${REUSE:-0}" != 1 ]; then bash scripts/remote/lineprof.sh "$name" 2000 . > /tmp/ss-lp.txt 2>&1; fi
grep -E "$re" /tmp/ss-lp.txt | head -"$top"
grep -E "$re" /tmp/ss-lp.txt | awk '{ gsub("%", "", $1); s += $1 } END { printf "%.2f%% in lines matching\n", s }'
