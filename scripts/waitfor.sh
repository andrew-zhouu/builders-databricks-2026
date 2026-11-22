#!/usr/bin/env bash
# Wait (up to SECONDS_MAX, default 560) until a file holds a line matching a
# pattern, then show the file's tail. For following a queued remote job from
# its local output file without polling by hand.
# Usage: waitfor.sh FILE PATTERN [SECONDS_MAX] [TAIL_LINES]
file="$1"; pattern="$2"; max="${3:-560}"; lines="${4:-25}"
for _ in $(seq $((max / 5))); do
  grep -q -E "$pattern" "$file" 2>/dev/null && break
  sleep 5
done
tail -n "$lines" "$file" 2>/dev/null | cut -c1-700
grep -q -E "$pattern" "$file" 2>/dev/null && echo "[matched]" || echo "[not yet]"
