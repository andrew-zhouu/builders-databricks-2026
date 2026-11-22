#!/usr/bin/env bash
# Is the default output of tools/fuzz.py still what the committed version
# (REV, default the commit this branch started from) writes? A few seeds, a
# few hundred rows: light enough to run anywhere.
# Usage: fuzzsame.sh [REV]
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
rev="${1:-ef60c11}"
W=$(mktemp -d)
git show "$rev:tools/fuzz.py" > "$W/old.py"
for seed in 3 77 1234; do
  python3 -I "$W/old.py" "$seed" "$W/a$seed" 400
  python3 -I tools/fuzz.py "$seed" "$W/b$seed" 400
  diff -rq "$W/a$seed" "$W/b$seed" > /dev/null || { echo "seed $seed: default output changed"; rm -rf "$W"; exit 1; }
  FUZZ_IDS=dense python3 -I tools/fuzz.py "$seed" "$W/c$seed" 400
  diff -rq "$W/a$seed" "$W/c$seed" > /dev/null && { echo "seed $seed: dense output equals the default"; rm -rf "$W"; exit 1; }
done
rm -rf "$W"
echo "fuzz.py: default output unchanged, dense output differs"
