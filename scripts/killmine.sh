#!/usr/bin/env bash
# Stop leftover background helpers of THIS project only (matched by full path).
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
for pat in "$PROJ/bin/icount" "$PROJ/scripts/when_quiet.sh" "$PROJ/scripts/icount.sh" "$PROJ/.toolchain/prof-target"; do
  pkill -u "$(id -u)" -f "$pat" 2>/dev/null && echo "stopped: $pat"
done
git -C "$PROJ" tag -l; git -C "$PROJ" branch
ps -eo pid,etimes,args | grep "$PROJ" | grep -v grep | grep -v killmine | cut -c1-120 | head
