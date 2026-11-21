#!/usr/bin/env bash
# The plain implementation on its own: fixtures, dev answer, and its speed.
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
BIN="$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions"
bash "$PROJ/tools/fixtures.sh" "$BIN" --slow
for name in dev "$@"; do
  out="$PROJ/out/slow-$name"; rm -rf "$out"; mkdir -p "$out"
  /usr/bin/time -f "plain $name: %es wall %Us user %MKB" "$BIN" --slow "$PROJ/data/$name" "$out"
  exp="$PROJ/expected/$name.slow"
  [ "$name" = dev ] && exp="$PROJ/official/dev-answer/x/dev-answer"
  "$PROJ/.venv/bin/python" -I "$PROJ/tools/cmp.py" "$exp" "$out" | tail -1
done
