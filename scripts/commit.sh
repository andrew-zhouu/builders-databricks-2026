#!/usr/bin/env bash
# Commit the working tree with the given message (local repo only; nothing is pushed).
set -euo pipefail
cd /home/andre/builders/databricks
if [ ! -d .git ]; then
  git init -q -b main
fi
git add -A
git -c user.name="${GIT_AUTHOR_NAME:-team}" -c user.email="${GIT_AUTHOR_EMAIL:team@users.noreply.github.com}" \
  commit -q -m "$1" || echo "nothing to commit"
git log --oneline | head -3
