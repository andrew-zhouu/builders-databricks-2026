#!/usr/bin/env bash
# Runs ON the remote machine: what the input of a data set looks like.
# Usage: peek.sh [DATASET]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="${1:-m20}"
ls -la "data/$name" | head -20
for d in "data/$name"/*/; do
  echo "== $d: $(ls "$d" | wc -l) files, $(du -sh "$d" | cut -f1)"
  ls -la "$d" | sed -n '2,6p'
  f=$(ls "$d"/* | head -1)
  echo "-- $f"; head -c 1200 "$f"; echo
  echo "-- middle"; tail -c +500000000 "$f" 2>/dev/null | head -c 700; echo
done
cat "data/$name/manifest.json" 2>/dev/null
