#!/usr/bin/env bash
# Does tools/mutate.py get through every mutation on awkward input? A few
# small sets from tools/fuzz.py (no generator, no key to infer), every
# mutation and a few mixes on each. Light enough to run anywhere; nothing is
# compared, a crash is what this looks for.
# Usage: mutsmoke.sh
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
W=$(mktemp -d)
fails=0
for seed in 5 6 7; do
  python3 -I tools/fuzz.py "$seed" "$W/base$seed" 600
  FUZZ_IDS=dense python3 -I tools/fuzz.py "$seed" "$W/dense$seed" 600
  FUZZ_IDS=long python3 -I tools/fuzz.py "$seed" "$W/long$seed" 600
  for base in "$W/base$seed" "$W/dense$seed" "$W/long$seed"; do
    for name in $(python3 -I tools/mutate.py list) mix mix,mix none; do
      if ! python3 -I tools/mutate.py "$seed" "$base" "$W/out" "$name" > "$W/log" 2>&1; then
        echo "FAIL $name on $(basename "$base"): $(tail -3 "$W/log" | tr '\n' ' ')"; fails=$((fails + 1))
      fi
    done
  done
done
rm -rf "$W"
echo "mutate.py smoke: $fails failures"
[ "$fails" = 0 ]
