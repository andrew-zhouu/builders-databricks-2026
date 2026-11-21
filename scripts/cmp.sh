#!/usr/bin/env bash
# Byte-exact compare of two output folders (paths relative to the project root).
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
cd "$PROJ"
"$PROJ/.venv/bin/python" -I "$PROJ/tools/cmp.py" "$1" "$2"
