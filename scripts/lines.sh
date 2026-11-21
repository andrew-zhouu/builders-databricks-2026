#!/usr/bin/env bash
# Show source lines by number: lines.sh FILE N...   (also lists the LLVM tools available)
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
f="$1"; shift
for n in "$@"; do printf '%s:%s: %s\n' "$f" "$n" "$(sed -n "${n}p" "$PROJ/fast/src/$f")"; done
if [ -n "${TOOLS:-}" ]; then
  d=$(dirname "$(find "$RUSTUP_HOME/toolchains" -name llvm-objdump | head -1)")
  ls "$d" | tr '\n' ' '; echo
  "$d/llvm-objdump" --help | grep -i -E 'inline|line-numbers' | head
fi
