#!/usr/bin/env bash
# Runs ON the remote machine: wait until the detached job NAME (remote.sh tmux)
# has ended, at most SECONDS seconds, then show the end of its log.
# Usage: waitjob.sh NAME [SECONDS] [LINES]
name="$1"; limit="${2:-540}"; lines="${3:-30}"
log="$HOME/db/out/logs/$name.log"
end=$(( $(date +%s) + limit ))
while tmux has-session -t "$name" 2>/dev/null && [ "$(date +%s)" -lt "$end" ]; do sleep 10; done
tmux has-session -t "$name" 2>/dev/null && echo "(job $name still running)" || echo "(job $name ended)"
tail -"$lines" "$log" | cut -c1-150
date -u +%H:%M:%S
