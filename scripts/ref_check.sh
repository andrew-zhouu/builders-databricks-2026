#!/usr/bin/env bash
# Validate the Python reference: all fixtures, then the dev set vs the published answer.
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
PY="$PROJ/.venv/bin/python"
bash "$PROJ/tools/fixtures.sh" "$PY" -I "$PROJ/reference/reference.py"
mkdir -p "$PROJ/expected/dev"
/usr/bin/time -f "reference on dev: %es %MKB" "$PY" -I "$PROJ/reference/reference.py" "$PROJ/data/dev" "$PROJ/expected/dev"
"$PY" -I "$PROJ/tools/cmp.py" "$PROJ/official/dev-answer/x/dev-answer" "$PROJ/expected/dev"
