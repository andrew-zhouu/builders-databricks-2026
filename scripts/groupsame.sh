#!/usr/bin/env bash
# Is `grouping` (lines of a file in another order) really without effect on the
# answer? A few small fuzz sets with long IDs, the Python reference on each
# before and after. Light enough to run anywhere.
# Usage: groupsame.sh
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
W=$(mktemp -d)
for seed in 3 8 21; do
  FUZZ_IDS=long python3 -I tools/fuzz.py "$seed" "$W/a" 900
  python3 -I tools/mutate.py "$seed" "$W/a" "$W/b" grouping > "$W/log"
  python3 -I reference/reference.py "$W/a" "$W/ra"
  python3 -I reference/reference.py "$W/b" "$W/rb"
  if diff -rq "$W/ra" "$W/rb" > /dev/null; then echo "seed $seed: same answer ($(cut -c1-150 "$W/log"))"; else echo "seed $seed: the answer CHANGED"; rm -rf "$W"; exit 1; fi
  # (awk 1: a last line without a newline is a line all the same.)
  for folder in events clicks; do
    cmp -s <(LC_ALL=C awk 1 "$W"/a/$folder/*.csv | LC_ALL=C sort) <(LC_ALL=C awk 1 "$W"/b/$folder/*.csv | LC_ALL=C sort) \
      || { echo "seed $seed: not the same lines in $folder"; rm -rf "$W"; exit 1; }
  done
  rm -rf "$W/a" "$W/b" "$W/ra" "$W/rb"
done
rm -rf "$W"
echo "grouping: the same lines, the same answer"
