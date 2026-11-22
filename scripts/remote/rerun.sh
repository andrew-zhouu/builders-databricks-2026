#!/usr/bin/env bash
# Runs ON the remote machine: look at a failing set kept by idxfuzz.sh and run
# one setting on it again, several times, to tell a steady failure from one
# that comes and goes (a race). Compares with the kept yardstick answer
# (KEPT_DIR/slow) each time.
# Usage: rerun.sh KEPT_DIR BINARY_NAME TIMES [VAR=VALUE ...]
#   env: LABEL=name   show the kept .cmp and .log of that setting first
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
kept="$1"; bin="$PROJ/bin/$2"; times="$3"; shift 3
PY="$PROJ/.venv/bin/python"
if [ -n "${LABEL:-}" ]; then
  echo "kept: $(ls "$kept" | tr '\n' ' ' | cut -c1-300)"
  echo "kept $LABEL.cmp: $(head -c 1500 "$kept/$LABEL.cmp" 2> /dev/null | tr '\n' '|' | cut -c1-1500)"
  echo "kept $LABEL.log: $(tail -c 800 "$kept/$LABEL.log" 2> /dev/null | tr '\n' '|' | cut -c1-800)"
  echo "kept output: $(ls -la "$kept/$LABEL" "$kept/$LABEL"/* 2> /dev/null | tr '\n' '|' | cut -c1-700)"
fi
ok=0; bad=0
for k in $(seq "$times"); do
  o="$kept/rerun"; rm -rf "$o"; mkdir -p "$o"
  # shellcheck disable=SC2086
  if ! env "$@" timeout 300 "$bin" "$kept/data" "$o" 2> "$o.log"; then
    bad=$((bad + 1)); echo "run $k: exit code; $(tail -3 "$o.log" | tr '\n' ' ' | cut -c1-500)"; continue
  fi
  if "$PY" -I tools/cmp.py "$kept/slow" "$o" > "$o.cmp" 2>&1; then ok=$((ok + 1))
  else bad=$((bad + 1)); echo "run $k: $(head -c 700 "$o.cmp" | tr '\n' '|')"; cp -r "$o" "$kept/rerun-bad-$k"; fi
done
echo "rerun $(basename "$bin") ($*) on $kept: $ok right, $bad wrong of $times"
