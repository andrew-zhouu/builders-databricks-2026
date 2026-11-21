#!/usr/bin/env bash
# Build the official generator (statically, musl + rust-lld) and make datasets.
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
cd "$PROJ"
mkdir -p "$PROJ/.toolchain/bin" "$PROJ/data"
if [ ! -x "$PROJ/.toolchain/bin/generate" ]; then
  CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
  cargo build --release --target x86_64-unknown-linux-musl \
    --manifest-path "$PROJ/official/generator/x/generator/Cargo.toml" \
    --target-dir "$PROJ/.toolchain/gen-target"
  cp "$PROJ/.toolchain/gen-target/x86_64-unknown-linux-musl/release/generate" "$PROJ/.toolchain/bin/generate"
fi
gen() { # name seed events clicks
  if [ ! -f "$PROJ/data/$1/manifest.json" ]; then
    /usr/bin/time -f "gen $1: %es %MKB" generate --seed "$2" --events "$3" --clicks "$4" --output "$PROJ/data/$1"
  fi
  printf '%s: ' "$1"; cat "$PROJ/data/$1/manifest.json" | tr -d '\n'; echo
  for d in events clicks links; do
    printf '  %-7s files=%s bytes=%s\n' "$d" "$(ls "$PROJ/data/$1/$d" | wc -l)" "$(du -sb "$PROJ/data/$1/$d" | cut -f1)"
  done
}
for spec in "$@"; do
  IFS=: read -r name seed events clicks <<<"$spec"
  gen "$name" "$seed" "$events" "$clicks"
done
