#!/usr/bin/env bash
# Runs ON the remote machine: the section profile (CPU ms and ns per row of the
# main sections, and wall time) of the current build under each given
# environment setting, or of a kept binary (BIN=name), one run each.
# Usage: sprof.sh DATASET [VAR=VALUE | BIN=name]...
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="$1"; shift
[ $# -gt 0 ] || set -- X=1
for setting in "$@"; do
  case "$setting" in
    BIN=*) BIN="$PROJ/bin/${setting#BIN=}" bash scripts/prof.sh "$name" 8 2>&1 ;;
    *) env "$setting" bash scripts/prof.sh "$name" 8 2>&1 ;;
  esac | awk -v s="$setting" '
    /parse events|parse clicks|dedup|sessions|write user|links|resolve/ { printf "%s %s %.0f/%.1f;", $1, $2, $3, $7 }
    /sections: cpu/ { cpu = $3 }
    /wall / { printf "\n   %-22s cpu %s ms, %s\n", s, cpu, $0 }' | sed 's/parse events/P.ev/; s/parse clicks/P.cl/; s/dedup /d./g; s/sessions /s./g; s/write user/wr/; s/resolve people/res/' | cut -c1-330
done
