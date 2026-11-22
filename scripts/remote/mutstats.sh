#!/usr/bin/env bash
# Runs ON the remote machine: generate one set, mutate it, and show what the
# mutation put there (tools/mutstats.py). Light: no pipeline is run.
# Usage: mutstats.sh SEED EVENTS MUTATIONS
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
seed="$1"; events="$2"; names="$3"
W="$PROJ/out/mutstats"; rm -rf "$W"; mkdir -p "$W"
generate --seed "$seed" --events "$events" --clicks $((events / 3)) --output "$W/base" --threads 2 > /dev/null 2>&1 || { echo "generator failed"; exit 1; }
"$PROJ/.venv/bin/python" -I tools/mutate.py "$seed" "$W/base" "$W/data" "$names" || exit 1
"$PROJ/.venv/bin/python" -I tools/mutstats.py "$W/base" | head -3
"$PROJ/.venv/bin/python" -I tools/mutstats.py "$W/data"
rm -rf "$W"
