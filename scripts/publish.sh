#!/usr/bin/env bash
# Push a branch to origin and prove that what GitHub now serves is byte for
# byte the validated tree: a fresh clone must have the same commit and tree.
# Usage: publish.sh BRANCH
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
cd "$PROJ"
branch="${1:-main}"
local_sha=$(git rev-parse "$branch")
local_tree=$(git rev-parse "$branch^{tree}")
GCM_INTERACTIVE=Never GIT_TERMINAL_PROMPT=0 git push -u origin "$branch" 2>&1 | tail -4
remote_sha=$(git ls-remote origin "refs/heads/$branch" | cut -f1)
echo "local  $branch $local_sha"
echo "remote $branch $remote_sha"
[ "$local_sha" = "$remote_sha" ] || { echo "MISMATCH"; exit 1; }
tmp=$(mktemp -d)
git clone -q --branch "$branch" "$(git remote get-url origin)" "$tmp/clone"
clone_tree=$(git -C "$tmp/clone" rev-parse 'HEAD^{tree}')
files=$(git -C "$tmp/clone" ls-files | wc -l)
rm -rf "$tmp"
echo "fresh clone tree $clone_tree ($files files); validated tree $local_tree"
[ "$clone_tree" = "$local_tree" ] && echo "PUBLISHED AND VERIFIED: $remote_sha"
echo "--- default branch on GitHub"; git ls-remote --symref origin HEAD | head -1
