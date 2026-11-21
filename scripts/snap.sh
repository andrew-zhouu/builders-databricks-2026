#!/usr/bin/env bash
# Save binaries for A/B runs (bin/ is not committed).
#   snap.sh NAME          copy the current build to bin/NAME
#   snap.sh NAME REV      build git revision REV in a temporary worktree into bin/NAME
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
set -euo pipefail
mkdir -p "$PROJ/bin"
name="$1"
if [ $# -ge 2 ]; then
  wt="$PROJ/.toolchain/wt-$name"
  rm -rf "$wt"; git -C "$PROJ" worktree prune
  git -C "$PROJ" worktree add -q --detach "$wt" "$2"
  (cd "$wt/fast" && cargo build --release --target-dir "$PROJ/.toolchain/wt-target" 2>&1 | tail -1)
  cp "$PROJ/.toolchain/wt-target/x86_64-unknown-linux-musl/release/sessions" "$PROJ/bin/$name"
  git -C "$PROJ" worktree remove --force "$wt"
else
  cp "$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions" "$PROJ/bin/$name"
fi
ls -la "$PROJ/bin/$name" | awk '{print "saved", $NF, $5, "bytes"}'
