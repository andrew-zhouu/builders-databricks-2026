#!/usr/bin/env bash
# Runs ON the remote machine: one short correctness-only script under the
# machine's lock, like `remote.sh run` (start it with `remote.sh quick`, which
# takes no lock itself). Only with GATE_OK=1, and while a gate holds the lock
# for a long time (a tmux session named like w2 or v1 exists), does it run
# without the lock: an exception the lead grants for the length of a gate.
# Usage: gated.sh SCRIPT [ARG...]
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
if [ -n "${GATE_OK:-}" ] && tmux ls 2>/dev/null | grep -q -E '^[a-z]+[0-9]+:'; then
  exec bash "$@"
else
  exec flock /tmp/ss-heavy.lock bash "$@"
fi
