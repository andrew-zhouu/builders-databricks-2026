#!/usr/bin/env bash
# The fuzz generator's modes, checked locally on small sets: the default output
# is what it was (scripts/fuzzsame.sh), FUZZ_IDS=long differs from it, and what
# IDs a long set holds (tools/idstats.py). Light enough to run anywhere.
# Usage: fuzzlong.sh [SEED] [ROWS]
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
seed="${1:-7}"; rows="${2:-1500}"
bash scripts/fuzzsame.sh
W=$(mktemp -d)
python3 -I tools/fuzz.py "$seed" "$W/plain" "$rows"
FUZZ_IDS=long python3 -I tools/fuzz.py "$seed" "$W/long" "$rows"
diff -rq "$W/plain" "$W/long" > /dev/null && { echo "long output equals the default"; rm -rf "$W"; exit 1; }
echo "FUZZ_IDS=long, seed $seed, $rows rows:"
python3 -I tools/idstats.py "$W/long"
rm -rf "$W"
