#!/usr/bin/env bash
# Runs ON the remote machine, in a secondary working tree's directory: restore
# the symbolic links to what it shares with the main tree (data sets, reference
# answers, toolchain, Python environment, official material). `remote.sh push`
# used to delete them: its exclusions were written for directories (a trailing
# slash), and a link is not one.
# Usage: relink.sh [MAIN_DIR]
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
main="${1:-$(dirname "$PWD")/db}"
for d in data expected .toolchain .venv official; do
  [ -e "$d" ] || ln -s "$main/$d" "$d"
done
mkdir -p out bin
for b in scored oracle tbl c8; do [ -e "bin/$b" ] || cp "$main/bin/$b" bin/ 2>/dev/null; done
ls -la | grep -- '->' ; ls bin
