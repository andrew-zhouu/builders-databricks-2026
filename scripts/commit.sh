#!/usr/bin/env bash
# Commit the working tree with the given message (local repo only; nothing is pushed).
# The author comes from git's own configuration (repo-local or global).
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
cd "$PROJ"
[ -e .git ] || { echo "not a git repository: $PROJ"; exit 1; }   # (a file in a secondary working tree)
git add -A
# No links in the repository: in a secondary tree they point into this machine.
if git ls-files -s | grep -q '^120000'; then
  echo "refusing to commit a symbolic link:"; git ls-files -s | grep '^120000'; exit 1
fi
git commit -q -m "$1" || echo "nothing to commit"
git log --oneline | head -3
