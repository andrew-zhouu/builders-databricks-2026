#!/usr/bin/env bash
# Runs ON the remote machine: the self-checks of the wide parser over many
# large fuzz inputs. No reference answers here (fuzz.sh has those, on small
# inputs): only the two paths of each parser stage against each other, batch
# by batch, on a hundred times more adversarial lines. SS_SIMD_CHECK=1 compares
# the wide decode and emit stages with the ordinary ones, SS_SPLIT_CHECK=1 the
# batch splitter with the line splitter; either stops the run at the first
# difference. Each input once with ordinary and once with small chunks.
# Usage: widefuzz.sh FIRST_SEED COUNT [ROWS]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
first="$1"; count="$2"; rows="${3:-30000}"
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
PY="$PROJ/.venv/bin/python"
W="/tmp/ss-widefuzz-$$"; mkdir -p "$W"
bad=0
for seed in $(seq "$first" $((first + count - 1))); do
  rm -rf "$W/data" "$W/out"
  "$PY" -I tools/fuzz.py "$seed" "$W/data" "$rows" > /dev/null || { echo "seed $seed: generator failed"; bad=$((bad + 1)); continue; }
  for chunk in "" $((seed % 3001 + 500)); do
    if ! env SS_SIMD_CHECK=1 SS_SPLIT_CHECK=1 ${chunk:+SS_CHUNK_BYTES=$chunk} NCPU=8 "$BIN" "$W/data" "$W/out" > "$W/log" 2>&1; then
      echo "FAIL seed $seed ${chunk:+chunk $chunk}"
      grep -m3 -A2 -E 'SS_SIMD_CHECK|panicked|differs' "$W/log" | cut -c1-300
      bad=$((bad + 1))
    fi
    rm -rf "$W/out"
  done
done
rm -rf "$W"
echo "wide fuzz seeds $first..$((first + count - 1)) rows=$rows: $bad failing"
[ "$bad" = 0 ]
