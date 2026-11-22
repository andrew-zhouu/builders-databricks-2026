#!/usr/bin/env bash
# Runs ON the remote machine: who is right on a data set? The Python reference,
# the plain implementation of YARD, and a binary's fast path (under the given
# settings) and its own plain implementation, each compared with the reference.
# Usage: judge.sh DATA_DIR BINARY_NAME [VAR=VALUE ...]
#   env: YARD=name in bin/ (default dev9)
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
data="$1"; bin="$PROJ/bin/$2"; shift 2
yard="$PROJ/bin/${YARD:-dev9}"
PY="$PROJ/.venv/bin/python"
W="$(mktemp -d "$PROJ/out/judge.XXXXXX")"
"$PY" -I reference/reference.py "$data" "$W/ref" 2> "$W/ref.log" || { echo "the reference fails: $(tail -1 "$W/ref.log")"; exit 1; }
verdict() { # label dir
  if "$PY" -I tools/cmp.py "$W/ref" "$2" > "$W/cmp.txt" 2>&1; then echo "same as the reference: $1"
  else echo "DIFFERS from the reference: $1: $(grep -m7 -E 'rows|A:|B:' "$W/cmp.txt" | tr '\n' ' ' | cut -c1-900)"; fi
}
mkdir -p "$W/yard" "$W/slow" "$W/fast"
"$yard" --slow "$data" "$W/yard" 2> "$W/yard.log" && verdict "$(basename "$yard") --slow" "$W/yard" || echo "$(basename "$yard") --slow fails: $(tail -1 "$W/yard.log")"
if [ "$bin" != "$yard" ]; then
  "$bin" --slow "$data" "$W/slow" 2> "$W/slow.log" && verdict "$(basename "$bin") --slow" "$W/slow" || echo "$(basename "$bin") --slow fails: $(tail -1 "$W/slow.log")"
fi
# shellcheck disable=SC2086
env "$@" "$bin" "$data" "$W/fast" 2> "$W/fast.log" && verdict "$(basename "$bin") fast ($*)" "$W/fast" || echo "$(basename "$bin") fast fails: $(tail -2 "$W/fast.log" | tr '\n' ' ' | cut -c1-300)"
rm -rf "$W"
