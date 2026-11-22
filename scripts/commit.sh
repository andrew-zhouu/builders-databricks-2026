#!/usr/bin/env bash
# Commit the working tree with the given message (local repo only; nothing is pushed).
# The author comes from git's own configuration (repo-local or global).
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
cd "$PROJ"
[ -e .git ] || { echo "not a git repository: $PROJ"; exit 1; }   # (a file in a secondary working tree)
git add -A
git commit -q -m "$1

Co-Authored-By: Claude Fable 5.2 <noreply@anthropic.com>" || echo "nothing to commit"
git log --oneline | head -3
