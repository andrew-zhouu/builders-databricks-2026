#!/usr/bin/env bash
# Runs ON the remote machine: one short correctness-only script under the
# machine's rule: while a gate holds the lock for a long time (a tmux session
# named w<N> exists) it runs without the lock; otherwise it takes the lock,
# like `remote.sh run`. Start it with `remote.sh quick`.
# Usage: gated.sh SCRIPT [ARG...]
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
if tmux ls 2>/dev/null | grep -q -E '^[a-z]+[0-9]+:'; then
  exec bash "$@"
else
  exec flock /tmp/ss-heavy.lock bash "$@"
fi
