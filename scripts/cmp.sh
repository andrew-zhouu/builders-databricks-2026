#!/usr/bin/env bash
# Byte-exact compare of two output folders (paths relative to the project root).
. /home/andre/builders/databricks/scripts/env.sh
cd "$PROJ"
"$PROJ/.venv/bin/python" -I "$PROJ/tools/cmp.py" "$1" "$2"
