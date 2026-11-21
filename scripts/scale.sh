#!/usr/bin/env bash
# Scale check: generate NAME (seed/events/clicks) if missing, make the plain
# implementation's answer once, then diff the fast binary against it.
# Usage: scale.sh NAME SEED EVENTS CLICKS [NCPU]
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
name="$1"; seed="$2"; events="$3"; clicks="$4"; ncpu="${5:-8}"
if [ ! -f "$PROJ/data/$name/manifest.json" ]; then
  /usr/bin/time -f "generate $name: %es %MKB" generate --seed "$seed" --events "$events" --clicks "$clicks" \
    --output "$PROJ/data/$name" --threads "${GEN_THREADS:-3}"
fi
du -sh "$PROJ/data/$name" | cut -f1
exp="$PROJ/expected/$name.slow"
if [ ! -d "$exp/user_daily" ]; then
  mkdir -p "$exp"
  /usr/bin/time -f "plain implementation $name: %es %MKB" "$BIN" --slow "$PROJ/data/$name" "$exp"
fi
out="$PROJ/out/$name"; rm -rf "$out"; mkdir -p "$out"
NCPU="$ncpu" SS_TIMING=1 /usr/bin/time -f "fast $name: %es wall, %U user, %S sys, %MKB" "$BIN" "$PROJ/data/$name" "$out" 2>"$out.log"
cat "$out.log"
"$PROJ/.venv/bin/python" -I "$PROJ/tools/cmp.py" "$exp" "$out"
