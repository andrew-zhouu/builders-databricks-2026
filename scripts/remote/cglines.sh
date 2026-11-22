#!/usr/bin/env bash
# Runs ON the remote machine: after cgasm.sh has profiled a build, one
# function's instructions per event row by source line (no new valgrind run).
# Usage: cglines.sh FUNCTION_REGEX [LINES] [DATASET]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
bash scripts/remote/cgfn.sh "$1" 0.02 2000 "${3:-dev}" 2>&1 | sed -n '/by source line/,/-- instructions/p' | head -"${2:-60}" | cut -c1-150
