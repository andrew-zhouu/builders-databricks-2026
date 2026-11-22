#!/usr/bin/env bash
# Runs ON the remote machine: idxfuzz.sh for one binary with each of the given
# mutations in turn, a few seeds each (the seed still decides the size).
# Usage: focus.sh BINARY FIRST_SEED SEEDS_EACH MUTATION...      env: SETTINGS
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
bin="$1"; first="$2"; each="$3"; shift 3
rc=0
for mutation in "$@"; do
  MUTATION="$mutation" bash scripts/remote/idxfuzz.sh "$bin" "$each" "$first" 2>&1 | cut -c1-480 | sed "s/^idxfuzz/[$mutation] idxfuzz/" || rc=1
  first=$((first + each))
done
exit $rc
