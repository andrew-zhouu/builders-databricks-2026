#!/usr/bin/env bash
# Runs ON the remote machine: generate one set, mutate it, and show where its
# lines' fields end against 64-byte windows (tools/winstats.py) and what IDs
# it holds (tools/idstats.py). Light: no pipeline is run.
# Usage: winstats.sh SEED EVENTS MUTATIONS
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
seed="$1"; events="$2"; names="$3"
W="$PROJ/out/winstats"; rm -rf "$W"; mkdir -p "$W"
generate --seed "$seed" --events "$events" --clicks $((events / 3)) --output "$W/base" --threads 2 > /dev/null 2>&1 || { echo "generator failed"; exit 1; }
"$PROJ/.venv/bin/python" -I tools/mutate.py "$seed" "$W/base" "$W/data" "$names" | cut -c1-400 || exit 1
echo "as generated:"; "$PROJ/.venv/bin/python" -I tools/winstats.py "$W/base" | cut -c1-260
echo "after [$names]:"; "$PROJ/.venv/bin/python" -I tools/winstats.py "$W/data" | cut -c1-260
"$PROJ/.venv/bin/python" -I tools/idstats.py "$W/data" | cut -c1-230
rm -rf "$W"
