#!/usr/bin/env bash
# Runs ON the remote machine: stop this tree's queued or running jobs whose
# command line holds the given text (a job waiting for the machine's lock is
# just another process). Only processes under this tree's directory name.
# Usage: cancel.sh TEXT
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
text="$1"; mine="$(basename "$PWD")"
for pid in $(pgrep -f -- "$text"); do
  [ "$pid" = "$$" ] && continue
  [ "$pid" = "$PPID" ] && continue
  line="$(tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null)"
  cwd="$(readlink "/proc/$pid/cwd" 2>/dev/null)"
  case "$cwd" in
    *"/$mine"|*"/$mine/"*) kill "$pid" 2>/dev/null && echo "stopped $pid: ${line:0:100}" ;;
  esac
done
exit 0
