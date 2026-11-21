#!/usr/bin/env bash
# Runs ON the remote machine: the section profile's lines for the writer and
# the session phase, for the current build under each given environment
# setting (one run each), and for a kept binary if named.
# Usage: wprof.sh DATASET [VAR=VALUE | BIN=name]...
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="$1"; shift
[ $# -gt 0 ] || set -- X=1
for setting in "$@"; do
  echo "== $setting"
  case "$setting" in
    BIN=*) BIN="$PROJ/bin/${setting#BIN=}" bash scripts/prof.sh "$name" 8 2>&1 ;;
    *) env "$setting" bash scripts/prof.sh "$name" 8 2>&1 ;;
  esac | grep -E 'write user_daily|sessions people|sessions gather|sections:|wall ' | cut -c1-100
done
