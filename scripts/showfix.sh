#!/usr/bin/env bash
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
# Print the raw bytes of fixture inputs and expected outputs (cat -A shows $ and ^M, ^I).
F=$PROJ/official/starter-duckdb/x/starter-duckdb/fixtures
for c in "$@"; do
  echo "################ $c"
  for f in events/part-00000.csv clicks/part-00000.csv links/part-00000.csv; do
    for g in "$F/$c/input/${f%/*}"/*.csv; do echo "--- ${g#$F/$c/input/}"; cat -A "$g"; done
  done | awk '!seen[$0]++'
  echo "--- expected user_daily"; cat -A "$F/$c/expected/user_daily.csv"
  echo "--- expected campaign_daily"; cat -A "$F/$c/expected/campaign_daily.csv"
done
