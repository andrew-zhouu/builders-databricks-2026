#!/usr/bin/env bash
# List anything in a directory tree that looks like a personal address, a key or
# a token. The patterns are written so that this file does not match itself.
# Usage: secretscan.sh DIR      exit 0 = nothing found
dir="${1:-.}"
pat='[A-Za-z0-9._%+-]+@(gmail|yahoo|outlook|hotmail|icloud|proton)[.](com|me)'
pat="$pat"'|BEGIN [A-Z ]*PRIVATE[ ]KEY'
pat="$pat"'|gh[pousr]_[A-Za-z0-9]{30,}'
pat="$pat"'|github_pat_[A-Za-z0-9_]{30,}'
pat="$pat"'|AKIA[0-9A-Z]{16}'
pat="$pat"'|xox[baprs]-[A-Za-z0-9-]{10,}'
pat="$pat"'|sk-[A-Za-z0-9]{32,}'
if grep -rInE "$pat" "$dir" --exclude-dir=.git | cut -c1-200; then :; fi
[ -z "$(grep -rIlE "$pat" "$dir" --exclude-dir=.git)" ]
