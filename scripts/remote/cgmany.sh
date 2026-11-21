#!/usr/bin/env bash
# Runs ON the remote machine: cgsrc.sh for several functions from one valgrind
# run (the first call profiles, the rest reuse it).
# Usage: cgmany.sh DATASET MAX_LINES FUNCTION_REGEX...
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="$1"; max="$2"; shift 2
first=1
for fn in "$@"; do
  if [ "$first" = 1 ] && [ "${REUSE:-0}" != 1 ]; then
    bash scripts/remote/cgsrc.sh "$fn" . "$max" "$name"
  else
    REUSE=1 bash scripts/remote/cgsrc.sh "$fn" . "$max" "$name"
  fi
  first=0
done
