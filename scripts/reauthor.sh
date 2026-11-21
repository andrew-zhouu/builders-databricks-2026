#!/usr/bin/env bash
# Set the repo-local git identity and rewrite the (unpublished) local history to
# use it, so no personal address is baked into commits before the repository
# goes public. Commits any pending changes first. Usage: reauthor.sh "Name" "email"
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
cd "$PROJ"
[ -d .git ] || { echo "not a git repository: $PROJ"; exit 1; }
if git remote | grep -q .; then
  echo "refusing: this repository has a remote; history may already be published"; exit 1
fi
git config user.name "$1"
git config user.email "$2"
git add -A
git commit -q -m "Path-independent helper scripts" || true
git -c core.editor=true rebase -q --root --exec "git commit -q --amend --no-edit --reset-author"
git log --format='%h %an <%ae> %s' | head -20
