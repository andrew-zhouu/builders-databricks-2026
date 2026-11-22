#!/usr/bin/env bash
# Runs ON the remote machine: what order are the lines of an events file in?
# Shows the user and timestamps of the first lines, and counts over a sample:
# how often a line has the same user as the line before it, how often the
# ingest time does not go backwards, how many distinct users the sample has.
# Usage: order.sh [DATASET] [LINES]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="${1:-m20}"; lines="${2:-200000}"
f=$(ls -S data/"$name"/events/*.csv | head -1)
small=$(ls -S data/"$name"/events/*.csv | tail -1)
echo "largest: $f ($(stat -c %s "$f") bytes); smallest: $small ($(stat -c %s "$small") bytes); files: $(ls data/"$name"/events | wc -l)"
head -16 "$f" | cut -d, -f2-5 | cut -c1-100
for g in "$f" "$small"; do
  head -"$lines" "$g" | tail -n +2 | awk -F, '
    { u = $2; gsub(/"/, "", u); t = $4; gsub(/"/, "", t)
      if (u == pu) same++
      if (t >= pt) fwd++
      if (!(u in seen)) { seen[u] = 1; nu++ }
      pu = u; pt = t; n++ }
    END { printf "%d lines: same user as the line before %.1f%%, ingest not going back %.1f%%, distinct users %d (%.1f lines each)\n", n, 100*same/n, 100*fwd/n, nu, n/nu }'
done
