#!/usr/bin/env bash
# Runs ON the remote machine: what is running, how loaded it is, and the tail
# of a tmux job's log. Also clears wait loops left behind by a dropped session.
# Usage: status.sh [JOB] [LINES]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
pkill -f 'while pgrep' 2>/dev/null
date -u '+%Y-%m-%d %H:%M:%S UTC'; uptime | sed 's/.*load/load/'
free -g | sed -n 2p | awk '{print "memory GB: total " $2 ", used " $3 ", available " $7}'
ps aux --sort=-%cpu | awk 'NR > 1 && $3 > 5 {print "busy: " $3 "% " $11 " " $12 " " $13 " " $14}' | cut -c1-120 | head -5
tmux ls 2>/dev/null | sed 's/^/tmux: /'
if [ -n "${1:-}" ]; then echo "--- $1"; tail -"${2:-8}" "out/logs/$1.log"; fi
