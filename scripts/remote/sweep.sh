#!/usr/bin/env bash
# Runs ON the remote machine: one profile line per value of an environment
# switch, for the sections matching a pattern.
# Usage: sweep.sh VAR 'SECTION_REGEX' VALUE...     env: DATASET (default m20), REPEAT (default 2)
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
var="$1"; pat="$2"; shift 2
for v in "$@"; do
  for _ in $(seq "${REPEAT:-2}"); do
    env "$var=$v" bash scripts/prof.sh "${DATASET:-m20}" 8 2>&1 | grep -E "$pat" | cut -c1-78 | sed "s/^/$var=$v /"
  done
done
