#!/usr/bin/env bash
# The state of several running batteries at once: for each background output
# file (named by its task ID, in this session's task folder) the digest of
# scripts/failsum.sh, last lines only; first a wait until the first one holds
# a line matching PATTERN, or MAX seconds.
# Usage: poll.sh TASKS_DIR PATTERN MAX ID...
dir="$1"; pattern="$2"; max="$3"; shift 3
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bash "$here/waitfor.sh" "$dir/$1.output" "$pattern" "$max" 0 > /dev/null
for id in "$@"; do
  echo "--- $id"
  bash "$here/failsum.sh" "$dir/$id.output" | cut -c1-600 | tail -7
done
